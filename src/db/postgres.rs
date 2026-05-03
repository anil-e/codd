use crate::models::connection::ConnectionDetails;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::time::Duration;

#[derive(Debug)]
pub enum PostgresError {
    Sqlx(sqlx::Error),
}

impl std::fmt::Display for PostgresError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlx(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PostgresError {}

impl From<sqlx::Error> for PostgresError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

pub async fn connect(details: &ConnectionDetails) -> Result<PgPool, PostgresError> {
    let mut options = PgConnectOptions::new()
        .host(&details.saved.host)
        .port(details.saved.port)
        .database(&details.saved.database)
        .username(&details.saved.username)
        .ssl_mode(PgSslMode::Prefer);

    if !details.password.is_empty() {
        options = options.password(&details.password);
    }

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await?;

    Ok(pool)
}

pub async fn test_connection(details: &ConnectionDetails) -> Result<(), PostgresError> {
    let pool = connect(details).await?;
    sqlx::query("select 1").execute(&pool).await?;
    pool.close().await;

    Ok(())
}
