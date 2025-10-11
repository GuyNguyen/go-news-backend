use mongodb::{bson::doc, options::ClientOptions, Client};
use rss::Channel;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::io::Write;
use std::time::Duration;
use chrono::Local;
use log::{info, error};

const CHECK_INTERVAL_SECONDS: u64 = 60 * 30;

#[derive(Debug, Serialize, Deserialize)]
struct RssItem {
    title: String,
    link: String,
    description: String,
    pub_date: String,
}

async fn fetch_and_store_feed() -> Result<(), Box<dyn Error>> {
    info!("Starting RSS feed fetch...");
    let content = reqwest::get("https://gome.at/feed")
        .await?
        .bytes()
        .await?;
    info!("Successfully fetched RSS feed.");

    let channel = Channel::read_from(&content[..])?;
    info!("Successfully parsed RSS channel.");

    let client_options = ClientOptions::parse("mongodb://localhost:27017").await?;
    let client = Client::with_options(client_options)?;
    let db = client.database("rss_feed_db");
    let collection = db.collection::<RssItem>("feed_items");
    info!("Connected to MongoDB.");

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
        } else {
            info!("Already exists: {}", rss_item.title);
        }
    }
    info!("Feed processing complete.");
    Ok(())
}

#[tokio::main]
async fn main() {
    env_logger::Builder::new()
        .format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] - {}",
                Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.level(),
                record.args()
            )
        })
        .filter(None, log::LevelFilter::Info)
        .init();

    loop {
        info!("Running periodic check for RSS feed updates...");

        if let Err(e) = fetch_and_store_feed().await {
            error!("An error occurred during the feed check: {}", e);
        }

        let sleep_duration = Duration::from_secs(CHECK_INTERVAL_SECONDS);
        info!("Check finished. Sleeping for {} minutes.", sleep_duration.as_secs_f64() / 60.0);

        tokio::time::sleep(sleep_duration).await;
    }
}