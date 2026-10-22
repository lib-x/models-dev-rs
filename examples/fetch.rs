use models_dev_rs::{Client, ModelFilter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let download = Client::new()?.fetch().await?;
    println!("{} providers", download.catalog.providers.len());
    let filter = ModelFilter::default();
    for entry in download.catalog.search(&filter).take(10) {
        println!(
            "{}/{}: {}",
            entry.provider.id, entry.model.id, entry.model.name
        );
    }
    Ok(())
}
