use std::{env, error::Error, net::SocketAddr, sync::Arc};
use tor_books::{repositories::InMemoryCatalog, services::PublicBookRepository};

/// This binary is a strictly loopback-only S0 demo. It has NO persistent account/money state.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if env::var("TOR_DEMO_MODE").as_deref() != Ok("1") {
        return Err("Set TOR_DEMO_MODE=1 only for the disposable public-catalog demo".into());
    }
    let bind=env::var("TOR_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr=bind.parse()?;
    if !address.ip().is_loopback() {
        return Err("S0 demo may only bind to loopback behind an HTTPS reverse proxy".into());
    }
    let catalog:Arc<dyn PublicBookRepository>=Arc::new(InMemoryCatalog::demo());
    let listener=tokio::net::TcpListener::bind(address).await?;
    println!("tor S0 demo listening at {address}");
    axum::serve(listener,tor_web::router(catalog)).await?;
    Ok(())
}
