use crate::db::ssh_tunnel::{self, TunnelGuard};
use crate::models::connection::ConnectionDetails;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx_core::Url;
use std::str::FromStr;
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
    validate_connect_target(details)?;

    let tunnel = if details.saved.ssh_tunnel.is_some() {
        Some(ssh_tunnel::start_tunnel(details).await?)
    } else {
        None
    };

    let endpoint = connect_endpoint(details, tunnel.as_ref());
    let options = connect_options(details, database, &endpoint)?;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await?;

    Ok(PostgresConnection { pool, tunnel })
}

fn validate_connect_target(details: &ConnectionDetails) -> Result<(), sqlx::Error> {
    if details.saved.ssh_tunnel.is_some() && details.saved.host.starts_with('/') {
        return Err(sqlx::Error::Configuration(
            "Unix socket hosts cannot be used through an SSH tunnel."
                .to_string()
                .into(),
        ));
    }

    Ok(())
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

fn connect_options(
    details: &ConnectionDetails,
    database: &str,
    endpoint: &ConnectEndpoint,
) -> Result<PgConnectOptions, sqlx::Error> {
    let url = pgpass_connection_url(details, database);
    let mut options = PgConnectOptions::from_str(url.as_str())?
        .host(&endpoint.host)
        .port(endpoint.port)
        .database(database)
        .username(&details.saved.username)
        .ssl_mode(PgSslMode::Prefer);

    if !details.password.is_empty() {
        options = options.password(&details.password);
    }

    Ok(options)
}

fn pgpass_connection_url(details: &ConnectionDetails, database: &str) -> Url {
    let mut url = Url::parse("postgres://").expect("static PostgreSQL URL to parse");
    url.query_pairs_mut()
        .append_pair("host", &details.saved.host)
        .append_pair("port", &details.saved.port.to_string())
        .append_pair("dbname", database)
        .append_pair("user", &details.saved.username);

    url
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectEndpoint, connect_endpoint, connect_endpoint_for_tunnel_port, connect_options,
        pgpass_connection_url, validate_connect_target,
    };
    use crate::models::connection::{
        ConnectionDetails, SavedConnection, SshAuthMethod, SshTunnelConfig,
    };

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

    #[test]
    fn pgpass_url_uses_the_postgres_target_before_ssh_tunneling() {
        let details = connection_details();
        let url = pgpass_connection_url(&details, "reporting");
        let parameters = url.query_pairs().into_owned().collect::<Vec<_>>();

        assert_eq!(
            parameters,
            vec![
                ("host".to_string(), "localhost".to_string()),
                ("port".to_string(), "5432".to_string()),
                ("dbname".to_string(), "reporting".to_string()),
                ("user".to_string(), "anil".to_string()),
            ]
        );
    }

    #[test]
    fn direct_unix_socket_hosts_remain_unix_sockets() {
        let mut details = connection_details();
        details.saved.host = "/var/run/postgresql".to_string();
        let endpoint = connect_endpoint_for_tunnel_port(&details, None);
        let options =
            connect_options(&details, "postgres", &endpoint).expect("connection options to build");

        assert_eq!(
            options.get_socket().map(|path| path.as_path()),
            Some(std::path::Path::new("/var/run/postgresql"))
        );
    }

    #[test]
    fn unix_socket_hosts_are_rejected_with_ssh_tunneling() {
        let mut details = connection_details();
        details.saved.host = "/var/run/postgresql".to_string();
        details.saved.ssh_tunnel = Some(SshTunnelConfig {
            host: "example.com".to_string(),
            port: 22,
            username: "anil".to_string(),
            auth_method: SshAuthMethod::Agent,
            private_key_path: String::new(),
            save_secret: false,
            host_key_fingerprint: None,
        });

        assert!(validate_connect_target(&details).is_err());
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
