use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use chrono::Local;
use futures::stream::TryStreamExt;
use log::{error, info};
use mongodb::{
    bson::doc,
    options::{ClientOptions},
    Client,
};
use rss::Channel;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

const CHECK_INTERVAL_SECONDS: u64 = 60 * 30; // 30 minutes
const MONGO_URI: &str = "mongodb://localhost:27017";
const DB_NAME: &str = "rss_feed_db";
const COLLECTION_NAME: &str = "feed_items";

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RssItem {
    title: String,
    link: String,
    description: String,
    pub_date: String,
    #[serde(default)] // Add this attribute
    posted: bool, // New field to track post status
}

#[derive(Deserialize)]
struct MarkPostedRequest {
    links: Vec<String>,
}

#[get("/health")]
async fn health_check() -> impl Responder {
    info!("GET /health endpoint called.");
    HttpResponse::Ok().body("Service is running")
}

#[post("/force-check")]
async fn force_check(db_client: web::Data<Client>) -> impl Responder {
    info!("POST /force-check endpoint called.");
    match fetch_and_store_feed(&db_client).await {
        Ok(_) => HttpResponse::Ok().body("Feed check completed successfully."),
        Err(e) => {
            error!("Manual check failed: {}", e);
            HttpResponse::InternalServerError().body(format!("Failed to check feed: {}", e))
        }
    }
}

#[get("/items")]
async fn get_items(db_client: web::Data<Client>) -> impl Responder {
    info!("GET /items endpoint called.");
    let collection = db_client
        .database(DB_NAME)
        .collection::<RssItem>(COLLECTION_NAME);

    let filter = doc! {};

    match collection.find(filter).await {
        Ok(cursor) => {
            let items: Vec<RssItem> = match cursor.try_collect().await {
                Ok(items) => items,
                Err(e) => {
                    error!("Error collecting items from cursor: {}", e);
                    return HttpResponse::InternalServerError().finish();
                }
            };
            HttpResponse::Ok().json(items)
        }
        Err(e) => {
            error!("Failed to fetch items from database: {}", e);
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[get("/items/unposted")]
async fn get_unposted_items(db_client: web::Data<Client>) -> impl Responder {
    info!("GET /items/unposted endpoint called.");
    let collection = db_client
        .database(DB_NAME)
        .collection::<RssItem>(COLLECTION_NAME);

    let filter = doc! { "posted": false };

    match collection.find(filter).await {
        Ok(cursor) => {
            let items: Vec<RssItem> = match cursor.try_collect().await {
                Ok(items) => items,
                Err(e) => {
                    error!("Error collecting items from cursor: {}", e);
                    return HttpResponse::InternalServerError().finish();
                }
            };
            HttpResponse::Ok().json(items)
        }
        Err(e) => {
            error!("Failed to fetch unposted items from database: {}", e);
            HttpResponse::InternalServerError().finish()
        }
    }
}

#[post("/items/mark-posted")]
async fn mark_items_posted(
    db_client: web::Data<Client>,
    req: web::Json<MarkPostedRequest>,
) -> impl Responder {
    info!(
        "POST /items/mark-posted endpoint called for {} links.",
        req.links.len()
    );
    let collection = db_client
        .database(DB_NAME)
        .collection::<RssItem>(COLLECTION_NAME);

    let filter = doc! { "link": { "$in": &req.links } };
    let update = doc! { "$set": { "posted": true } };

    match collection.update_many(filter, update).await {
        Ok(result) => {
            info!("Successfully marked {} items as posted.", result.modified_count);
            HttpResponse::Ok().json(doc! {
                "message": "Update successful",
                "items_updated": result.modified_count as i64
            })
        }
        Err(e) => {
            error!("Failed to mark items as posted: {}", e);
            HttpResponse::InternalServerError().body("Failed to update items")
        }
    }
}

async fn fetch_and_store_feed(client: &Client) -> Result<(), Box<dyn Error>> {
    info!("Starting RSS feed fetch...");
    let content = reqwest::get("https://gome.at/feed").await?.bytes().await?;
    info!("Successfully fetched RSS feed.");

    let channel = Channel::read_from(&content[..])?;
    let collection = client
        .database(DB_NAME)
        .collection::<RssItem>(COLLECTION_NAME);
    info!("Successfully parsed RSS channel.");

    for item in channel.into_items() {
        let rss_item = RssItem {
            title: item.title().unwrap_or_default().to_string(),
            link: item.link().unwrap_or_default().to_string(),
            description: item.description().unwrap_or_default().to_string(),
            pub_date: item.pub_date().unwrap_or_default().to_string(),
            posted: false,
        };

        let filter = doc! { "link": &rss_item.link };
        let existing_item = collection.find_one(filter).await?;

        if existing_item.is_none() {
            collection.insert_one(&rss_item).await?;
            info!("Stored new item: {}", rss_item.title);
        }
    }
    info!("Feed processing complete.");
    Ok(())
}

async fn run_periodic_checker(client: web::Data<Client>) {
    let mut interval = tokio::time::interval(Duration::from_secs(CHECK_INTERVAL_SECONDS));
    loop {
        interval.tick().await; // Wait for the next tick
        info!("Running periodic check for RSS feed updates...");
        if let Err(e) = fetch_and_store_feed(&client).await {
            error!("An error occurred during the periodic feed check: {}", e);
        }
    }
}

fn setup_logger() -> Result<(), fern::InitError> {
    // Create a shared formatter
    let formatter = move |out: fern::FormatCallback, message: &std::fmt::Arguments, record: &log::Record| {
        out.finish(format_args!(
            "{} [{}] - {}",
            Local::now().format("%Y-%m-%dT%H:%M:%S"),
            record.level(),
            message
        ))
    };

    let console_log = fern::Dispatch::new()
        .format(formatter.clone())
        .level(log::LevelFilter::Info) // Set the log level for console
        .chain(std::io::stdout());

    let file_log = fern::Dispatch::new()
        .format(formatter)
        .level(log::LevelFilter::Info) // Set the log level for file
        .chain(fern::log_file("backend.log")?);

    fern::Dispatch::new()
        .chain(console_log)
        .chain(file_log)
        .apply()?;

    Ok(())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    setup_logger().expect("Failed to initialize logger.");

    info!("Connecting to MongoDB...");
    let client_options = ClientOptions::parse(MONGO_URI)
        .await
        .expect("Failed to parse MongoDB URI");
    let client = Client::with_options(client_options).expect("Failed to connect to MongoDB");
    let db_client = web::Data::new(client);
    info!("Successfully connected to MongoDB.");

    let background_client = db_client.clone();
    tokio::spawn(async move {
        run_periodic_checker(background_client).await;
    });
    info!("Periodic feed checker started in the background.");

    info!("Starting Actix web server at http://127.0.0.1:8080");
    HttpServer::new(move || {
        App::new()
            .app_data(db_client.clone())
            .service(health_check)
            .service(force_check)
            .service(get_items)
            .service(get_unposted_items)
            .service(mark_items_posted)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}