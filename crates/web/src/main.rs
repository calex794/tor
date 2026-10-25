use std::{env, error::Error, net::SocketAddr, sync::Arc};
use tor_books::{
    repositories::{InMemoryCatalog, PgCatalog},
    services::PublicBookRepository,
};
use tor_reading::repositories::PgReader;

/// No schema changes happen during HTTP startup.
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind = env::var("TOR_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let address: SocketAddr = bind.parse()?;
    if !address.ip().is_loopback() {
        return Err("TOR must bind to loopback behind HTTPS reverse proxy".into());
    }
    let demo = env::var("TOR_DEMO_MODE").as_deref() == Ok("1");
    let app = match env::var("DATABASE_URL") {
        Ok(url) if !demo => {
            let catalog = PgCatalog::connect(&url).await?;
            let reader = PgReader::connect(&url).await?;
            println!("TOR PostgreSQL catalogue and public reading on {address}");
            tor_web::router_with_postgres_reading(catalog, reader)
        }
        Ok(_) => return Err("Use either DATABASE_URL or TOR_DEMO_MODE=1".into()),
        Err(env::VarError::NotPresent) if demo => {
            let catalog: Arc<dyn PublicBookRepository> = Arc::new(InMemoryCatalog::demo());
            println!("tor S0 demo listening at {address}");
            tor_web::router(catalog)
        }
        Err(env::VarError::NotPresent) => {
            return Err("Set DATABASE_URL for PostgreSQL, or TOR_DEMO_MODE=1 for demo".into());
        }
        Err(error) => return Err(error.into()),
    };
    let listener = tokio::net::TcpListener::bind(address).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
