use gettextrs::gettext;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedConnection {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    #[serde(default)]
    pub save_password: bool,
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
    pub save_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionDetails {
    pub saved: SavedConnection,
    pub password: String,
}

impl ConnectionDetails {
    pub fn with_database(&self, database: impl Into<String>) -> Self {
        let mut details = self.clone();
        details.saved.database = database.into();

        details
    }
}

impl Default for ConnectionForm {
    fn default() -> Self {
        Self {
            id: None,
            name: String::new(),
            host: "localhost".to_string(),
            port: "5432".to_string(),
            database: "postgres".to_string(),
            username: String::new(),
            password: String::new(),
            save_password: false,
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
            save_password: connection.save_password,
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
            return Err(gettext("Default database is required."));
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
                save_password: self.save_password,
            },
            password: self.password.clone(),
        })
    }
}

fn connection_id() -> String {
    format!("pg-{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::{ConnectionForm, SavedConnection};

    #[test]
    fn saved_connection_defaults_save_password_for_legacy_json() {
        let connection: SavedConnection = serde_json::from_str(
            r#"{
                "id": "pg-1",
                "name": "Local",
                "host": "localhost",
                "port": 5432,
                "database": "postgres",
                "username": "anil"
            }"#,
        )
        .expect("legacy connection json to deserialize");

        assert!(!connection.save_password);
    }

    #[test]
    fn connection_form_copies_save_password_from_saved_connection() {
        let connection = SavedConnection {
            id: "pg-1".to_string(),
            name: "Local".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "postgres".to_string(),
            username: "anil".to_string(),
            save_password: true,
        };

        let form = ConnectionForm::from_saved(&connection);

        assert!(form.save_password);
        assert!(form.password.is_empty());
    }

    #[test]
    fn validated_details_preserve_save_password_without_storing_password() {
        let form = ConnectionForm {
            id: Some("pg-1".to_string()),
            name: " Local ".to_string(),
            host: " localhost ".to_string(),
            port: "5432".to_string(),
            database: " postgres ".to_string(),
            username: " anil ".to_string(),
            password: "secret".to_string(),
            save_password: true,
        };

        let details = form.validate().expect("form to validate");

        assert!(details.saved.save_password);
        assert_eq!(details.password, "secret");
    }
}
