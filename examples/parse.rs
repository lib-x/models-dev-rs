use std::{env, fs::File, io::Read};

use models_dev_rs::{Catalog, DEFAULT_MAX_BYTES, ModelFilter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args_os()
        .nth(1)
        .ok_or("usage: cargo run --example parse -- api.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(DEFAULT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let catalog = Catalog::from_slice(&bytes)?;
    let filter = ModelFilter::default();
    println!(
        "{} providers, {} models",
        catalog.providers.len(),
        catalog.search(&filter).count()
    );
    Ok(())
}
