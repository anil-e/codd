use crate::db::ssh_tunnel::{self, TunnelGuard};
use crate::models::connection::ConnectionDetails;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use std::time::Duration;

#[derive(Debug)]
pub enum PostgresError {
    Sqlx(sqlx::Error),
    SshTunnel(ssh_tunnel::SshTunnelError),
}

#[derive(Debug)]
pub struct PostgresConnection {
    pub pool: PgPool,
    pub tunnel: Option<TunnelGuard>,
}

impl std::fmt::Display for PostgresError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlx(error) => write!(formatter, "{error}"),
            Self::SshTunnel(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PostgresError {}

impl From<sqlx::Error> for PostgresError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

impl From<ssh_tunnel::SshTunnelError> for PostgresError {
    fn from(error: ssh_tunnel::SshTunnelError) -> Self {
        Self::SshTunnel(error)
    }
}

pub async fn connect(details: &ConnectionDetails) -> Result<PostgresConnection, PostgresError> {
    connect_to_database(details, &details.saved.database).await
}

pub async fn connect_to_database(
    details: &ConnectionDetails,
    database: &str,
) -> Result<PostgresConnection, PostgresError> {
    let tunnel = if details.saved.ssh_tunnel.is_some() {
        Some(ssh_tunnel::start_tunnel(details).await?)
    } else {
        None
    };

    let endpoint = connect_endpoint(details, tunnel.as_ref());
    let mut options = PgConnectOptions::new()
        .host(&endpoint.host)
        .port(endpoint.port)
        .database(database)
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

    Ok(PostgresConnection { pool, tunnel })
}

pub async fn list_databases(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT datname
        FROM pg_database
        WHERE datallowconn
          AND NOT datistemplate
        ORDER BY datname
        "#,
    )
    .fetch_all(pool)
    .await
}

pub async fn test_connection(details: &ConnectionDetails) -> Result<(), PostgresError> {
    let connection = connect(details).await?;
    sqlx::query("SELECT 1").execute(&connection.pool).await?;
    connection.pool.close().await;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectEndpoint {
    host: String,
    port: u16,
}

fn connect_endpoint(details: &ConnectionDetails, tunnel: Option<&TunnelGuard>) -> ConnectEndpoint {
    connect_endpoint_for_tunnel_port(details, tunnel.map(TunnelGuard::local_port))
}

fn connect_endpoint_for_tunnel_port(
    details: &ConnectionDetails,
    tunnel_port: Option<u16>,
) -> ConnectEndpoint {
    match tunnel_port {
        Some(port) => ConnectEndpoint {
            host: "127.0.0.1".to_string(),
            port,
        },
        None => ConnectEndpoint {
            host: details.saved.host.clone(),
            port: details.saved.port,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectEndpoint, connect_endpoint, connect_endpoint_for_tunnel_port};
    use crate::models::connection::{ConnectionDetails, SavedConnection};

    #[test]
    fn connect_endpoint_uses_saved_host_without_tunnel() {
        let details = ConnectionDetails {
            saved: SavedConnection {
                id: "pg-1".to_string(),
                name: "Local".to_string(),
                host: "localhost".to_string(),
                port: 5432,
                database: "postgres".to_string(),
                username: "anil".to_string(),
                save_password: false,
                ssh_tunnel: None,
            },
            password: String::new(),
            ssh_password: String::new(),
            ssh_key_passphrase: String::new(),
        };

        let endpoint = connect_endpoint(&details, None);

        assert_eq!(
            endpoint,
            ConnectEndpoint {
                host: "localhost".to_string(),
                port: 5432,
            }
        );
    }

    #[test]
    fn connect_endpoint_uses_localhost_with_tunnel_port() {
        let details = connection_details();
        let endpoint = connect_endpoint_for_tunnel_port(&details, Some(15432));

        assert_eq!(
            endpoint,
            ConnectEndpoint {
                host: "127.0.0.1".to_string(),
                port: 15432,
            }
        );
    }

    fn connection_details() -> ConnectionDetails {
        ConnectionDetails {
            saved: SavedConnection {
                id: "pg-1".to_string(),
                name: "Local".to_string(),
                host: "localhost".to_string(),
                port: 5432,
                database: "postgres".to_string(),
                username: "anil".to_string(),
                save_password: false,
                ssh_tunnel: None,
            },
            password: String::new(),
            ssh_password: String::new(),
            ssh_key_passphrase: String::new(),
        }
    }
}
