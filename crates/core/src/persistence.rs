use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

use crate::adapters::config::db_path;
use anyhow::{Ok, Result};

pub async fn get_db() -> Result<SqlitePool> {
    let db = db_path().expect("db path to be found");
    let options = SqliteConnectOptions::new().filename(db).create_if_missing(true);
    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await?;

    Ok(pool)
}
