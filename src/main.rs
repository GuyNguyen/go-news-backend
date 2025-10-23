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

// --- Configuration ---
const CHECK_INTERVAL_SECONDS: u64 = 60 * 30; // 30 minutes
const MONGO_URI: &str = "mongodb://localhost:27017";
const DB_NAME: &str = "rss_feed_db";
const COLLECTION_NAME: &str = "feed_items";

// --- Data Structure ---
#[derive(Debug, Serialize, Deserialize, Clone)]
struct RssItem {
    title: String,
    link: String,
    description: String,
    pub_date: String,
}

// --- API Handlers ---

/// Health check endpoint.
#[get("/health")]
async fn health_check() -> impl Responder {
    info!("GET /health endpoint called.");
    HttpResponse::Ok().body("Service is running")
}

/// Triggers a manual fetch of the RSS feed.
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

/// Fetches all stored items from the database.
#[get("/items")]
async fn get_items(db_client: web::Data<Client>) -> impl Responder {
    info!("GET /items endpoint called.");
    let collection = db_client
        .database(DB_NAME)
        .collection::<RssItem>(COLLECTION_NAME);

    // The filter for "all documents" is an empty document.
    let filter = doc! {};

    // Per the compiler error, `find` must be called with a single `Document` argument.
    // We have removed sorting for now to resolve the immediate compilation error.
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

// --- Core Logic ---

/// Fetches the feed, parses it, and stores new items in MongoDB.
/// This function is now reusable and accepts a client reference.
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
        };

        let filter = doc! { "link": &rss_item.link };
        let existing_item = collection.find_one(filter).await?;

        if existing_item.is_none() {
            collection.insert_one(&rss_item).await?;
            info!("Stored: {}", rss_item.title);
        }
    }
    info!("Feed processing complete.");
    Ok(())
}

/// The background task that runs periodically.
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

/// Sets up the logger to output to both console and file.
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

    // Log to console (stdout)
    let console_log = fern::Dispatch::new()
        .format(formatter.clone())
        .level(log::LevelFilter::Info) // Set the log level for console
        .chain(std::io::stdout());

    // Log to file (backend.log)
    let file_log = fern::Dispatch::new()
        .format(formatter)
        .level(log::LevelFilter::Info) // Set the log level for file
        .chain(fern::log_file("backend.log")?);

    // Apply both console and file loggers
    fern::Dispatch::new()
        .chain(console_log)
        .chain(file_log)
        .apply()?;

    Ok(())
}

// --- Main Application Setup ---
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // 1. Initialize Logger
    setup_logger().expect("Failed to initialize logger.");

    // 2. Connect to MongoDB and create shared data
    info!("Connecting to MongoDB...");
    let client_options = ClientOptions::parse(MONGO_URI)
        .await
        .expect("Failed to parse MongoDB URI");
    let client = Client::with_options(client_options).expect("Failed to connect to MongoDB");
    let db_client = web::Data::new(client);
    info!("Successfully connected to MongoDB.");

    // 3. Spawn the background task
    let background_client = db_client.clone();
    tokio::spawn(async move {
        run_periodic_checker(background_client).await;
    });
    info!("Periodic feed checker started in the background.");

    // 4. Start the Actix Web Server
    info!("Starting Actix web server at http://127.0.0.1:8080");
    HttpServer::new(move || {
        App::new()
            .app_data(db_client.clone()) // Share the DB client with handlers
            .service(health_check)
            .service(force_check)
            .service(get_items)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}