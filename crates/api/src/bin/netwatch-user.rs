use std::io::{self, BufRead, Read};

use anyhow::{bail, Context, Result};
use netwatch_crypto::password;
use sqlx::PgPool;
use zeroize::Zeroizing;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 || !matches!(args[2].as_str(), "admin" | "viewer") {
        bail!("usage: netwatch-user USERNAME admin|viewer < password-file");
    }
    let username = args[1].trim();
    if username.is_empty() || username.len() > 128 {
        bail!("username must contain 1 to 128 bytes");
    }
    let mut secret = Zeroizing::new(String::new());
    io::stdin()
        .lock()
        .take(1026)
        .read_line(&mut secret)
        .context("could not read password from stdin")?;
    while secret.ends_with(['\n', '\r']) {
        secret.pop();
    }
    if !(12..=1024).contains(&secret.len()) {
        bail!("password must contain 12 to 1024 bytes");
    }
    let hash = password::hash(secret.as_bytes())?;
    let pool = PgPool::connect(&std::env::var("DATABASE_URL").context("DATABASE_URL is required")?)
        .await?;
    sqlx::query("INSERT INTO local_users (username, password_hash, role) VALUES ($1, $2, $3)")
        .bind(username)
        .bind(hash)
        .bind(&args[2])
        .execute(&pool)
        .await?;
    println!("Created user {username}.");
    Ok(())
}
