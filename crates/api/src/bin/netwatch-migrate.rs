use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<()> {
    let url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    sqlx::migrate!("../db/migrations").run(&pool).await?;
    println!("Database migrations complete");
    Ok(())
}
