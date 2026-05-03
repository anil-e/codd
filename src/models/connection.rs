use gettextrs::gettext;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionForm {
    pub id: Option<String>,
    pub name: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionDetails {
    pub saved: SavedConnection,
    pub password: String,
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            host: "localhost".to_string(),
            port: "5432".to_string(),
            database: String::new(),
            username: String::new(),
            password: String::new(),
        }
    }
}

impl ConnectionForm {
    pub fn from_saved(connection: &SavedConnection) -> Self {
        Self {
            id: Some(connection.id.clone()),
            name: connection.name.clone(),
            host: connection.host.clone(),
            port: connection.port.to_string(),
            database: connection.database.clone(),
            username: connection.username.clone(),
            password: String::new(),
        }
    }

    pub fn validate(&self) -> Result<ConnectionDetails, String> {
        let name = self.name.trim();
        let host = self.host.trim();
        let port = self.port.trim();
        let database = self.database.trim();
        let username = self.username.trim();

        if name.is_empty() {
            return Err(gettext("Connection name is required."));
        }

        if host.is_empty() {
            return Err(gettext("Host is required."));
        }

        let port = port
            .parse::<u16>()
            .map_err(|_| gettext("Port must be a number between 1 and 65535."))?;

        if database.is_empty() {
            return Err(gettext("Database is required."));
        }

        if username.is_empty() {
            return Err(gettext("Username is required."));
        }

        Ok(ConnectionDetails {
            saved: SavedConnection {
                id: self.id.clone().unwrap_or_else(connection_id),
                name: name.to_string(),
                host: host.to_string(),
                port,
                database: database.to_string(),
                username: username.to_string(),
            },
            password: self.password.clone(),
        })
    }
}

fn connection_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();

    format!("pg-{millis}")
}
