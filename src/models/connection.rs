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
    #[serde(default)]
    pub ssh_tunnel: Option<SshTunnelConfig>,
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
    pub ssh_enabled: bool,
    pub ssh_host: String,
    pub ssh_port: String,
    pub ssh_username: String,
    pub ssh_auth_method: SshAuthMethod,
    pub ssh_password: String,
    pub ssh_private_key_path: String,
    pub ssh_key_passphrase: String,
    pub ssh_save_secret: bool,
    pub ssh_host_key_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionDetails {
    pub saved: SavedConnection,
    pub password: String,
    pub ssh_password: String,
    pub ssh_key_passphrase: String,
}

impl ConnectionDetails {
    pub fn with_database(&self, database: impl Into<String>) -> Self {
        let mut details = self.clone();
        details.saved.database = database.into();

        details
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshTunnelConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth_method: SshAuthMethod,
    #[serde(default)]
    pub private_key_path: String,
    #[serde(default)]
    pub save_secret: bool,
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SshAuthMethod {
    #[default]
    Password,
    PrivateKey,
    Agent,
}

impl SshAuthMethod {
    pub fn label(self) -> String {
        match self {
            Self::Password => gettext("Password"),
            Self::PrivateKey => gettext("Private Key"),
            Self::Agent => gettext("SSH Agent"),
        }
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
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: "22".to_string(),
            ssh_username: String::new(),
            ssh_auth_method: SshAuthMethod::Password,
            ssh_password: String::new(),
            ssh_private_key_path: String::new(),
            ssh_key_passphrase: String::new(),
            ssh_save_secret: false,
            ssh_host_key_fingerprint: None,
        }
    }
}

impl ConnectionForm {
    pub fn from_saved(connection: &SavedConnection) -> Self {
        let ssh_tunnel = connection.ssh_tunnel.as_ref();

        Self {
            id: Some(connection.id.clone()),
            name: connection.name.clone(),
            host: connection.host.clone(),
            port: connection.port.to_string(),
            database: connection.database.clone(),
            username: connection.username.clone(),
            password: String::new(),
            save_password: connection.save_password,
            ssh_enabled: ssh_tunnel.is_some(),
            ssh_host: ssh_tunnel
                .map(|config| config.host.clone())
                .unwrap_or_default(),
            ssh_port: ssh_tunnel
                .map(|config| config.port.to_string())
                .unwrap_or_else(|| "22".to_string()),
            ssh_username: ssh_tunnel
                .map(|config| config.username.clone())
                .unwrap_or_default(),
            ssh_auth_method: ssh_tunnel
                .map(|config| config.auth_method)
                .unwrap_or_default(),
            ssh_password: String::new(),
            ssh_private_key_path: ssh_tunnel
                .map(|config| config.private_key_path.clone())
                .unwrap_or_default(),
            ssh_key_passphrase: String::new(),
            ssh_save_secret: ssh_tunnel.is_some_and(|config| config.save_secret),
            ssh_host_key_fingerprint: ssh_tunnel
                .and_then(|config| config.host_key_fingerprint.clone()),
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

        let ssh_tunnel = if self.ssh_enabled {
            Some(self.validate_ssh_tunnel()?)
        } else {
            None
        };

        Ok(ConnectionDetails {
            saved: SavedConnection {
                id: self.id.clone().unwrap_or_else(connection_id),
                name: name.to_string(),
                host: host.to_string(),
                port,
                database: database.to_string(),
                username: username.to_string(),
                save_password: self.save_password,
                ssh_tunnel,
            },
            password: self.password.clone(),
            ssh_password: self.ssh_password.clone(),
            ssh_key_passphrase: self.ssh_key_passphrase.clone(),
        })
    }

    fn validate_ssh_tunnel(&self) -> Result<SshTunnelConfig, String> {
        let host = self.ssh_host.trim();
        let port = self.ssh_port.trim();
        let username = self.ssh_username.trim();
        let private_key_path = self.ssh_private_key_path.trim();

        if host.is_empty() {
            return Err(gettext("SSH host is required."));
        }

        let port = port
            .parse::<u16>()
            .map_err(|_| gettext("SSH port must be a number between 1 and 65535."))?;

        if username.is_empty() {
            return Err(gettext("SSH username is required."));
        }

        if self.ssh_auth_method == SshAuthMethod::PrivateKey && private_key_path.is_empty() {
            return Err(gettext("Private key file is required."));
        }

        let save_secret = match self.ssh_auth_method {
            SshAuthMethod::Password | SshAuthMethod::PrivateKey => self.ssh_save_secret,
            SshAuthMethod::Agent => false,
        };
        let private_key_path = match self.ssh_auth_method {
            SshAuthMethod::PrivateKey => private_key_path.to_string(),
            SshAuthMethod::Password | SshAuthMethod::Agent => String::new(),
        };

        Ok(SshTunnelConfig {
            host: host.to_string(),
            port,
            username: username.to_string(),
            auth_method: self.ssh_auth_method,
            private_key_path,
            save_secret,
            host_key_fingerprint: self.ssh_host_key_fingerprint.clone(),
        })
    }
}

fn connection_id() -> String {
    format!("pg-{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::{ConnectionForm, SavedConnection, SshAuthMethod, SshTunnelConfig};

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
        assert!(connection.ssh_tunnel.is_none());
    }

    #[test]
    fn connection_form_copies_save_password_from_saved_connection() {
        let connection = SavedConnection {
            save_password: true,
            ..saved_connection()
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
            ..ConnectionForm::default()
        };

        let details = form.validate().expect("form to validate");

        assert!(details.saved.save_password);
        assert_eq!(details.password, "secret");
        assert!(details.saved.ssh_tunnel.is_none());
    }

    #[test]
    fn connection_form_copies_ssh_fields_without_secrets() {
        let connection = SavedConnection {
            name: "Remote".to_string(),
            host: "db.internal".to_string(),
            username: "postgres".to_string(),
            ssh_tunnel: Some(private_key_tunnel_config()),
            ..saved_connection()
        };

        let form = ConnectionForm::from_saved(&connection);

        assert!(form.ssh_enabled);
        assert_eq!(form.ssh_host, "bastion.example.com");
        assert_eq!(form.ssh_auth_method, SshAuthMethod::PrivateKey);
        assert!(form.ssh_password.is_empty());
        assert!(form.ssh_key_passphrase.is_empty());
    }

    #[test]
    fn saved_connection_ignores_legacy_ssh_enabled_field() {
        let connection: SavedConnection = serde_json::from_str(
            r#"{
                "id": "pg-1",
                "name": "Remote",
                "host": "db.internal",
                "port": 5432,
                "database": "postgres",
                "username": "postgres",
                "ssh_tunnel": {
                    "enabled": true,
                    "host": "bastion.example.com",
                    "port": 22,
                    "username": "anil",
                    "auth_method": "password"
                }
            }"#,
        )
        .expect("connection json with legacy ssh enabled field to deserialize");

        assert!(connection.ssh_tunnel.is_some());
    }

    #[test]
    fn ssh_private_key_requires_key_path() {
        let form = ConnectionForm {
            ssh_enabled: true,
            ssh_host: "bastion.example.com".to_string(),
            ssh_username: "anil".to_string(),
            ssh_auth_method: SshAuthMethod::PrivateKey,
            ..ConnectionForm::default()
        };

        assert!(form.validate().is_err());
    }

    #[test]
    fn ssh_agent_auth_method_serializes_as_agent() {
        let serialized =
            serde_json::to_string(&SshAuthMethod::Agent).expect("auth method to serialize");
        let deserialized: SshAuthMethod =
            serde_json::from_str(&serialized).expect("auth method to deserialize");

        assert_eq!(serialized, r#""agent""#);
        assert_eq!(deserialized, SshAuthMethod::Agent);
    }

    #[test]
    fn ssh_agent_auth_validates_without_private_key_path() {
        let form = ConnectionForm {
            name: "Remote".to_string(),
            username: "postgres".to_string(),
            ssh_enabled: true,
            ssh_host: "bastion.example.com".to_string(),
            ssh_username: "anil".to_string(),
            ssh_auth_method: SshAuthMethod::Agent,
            ..ConnectionForm::default()
        };

        let details = form.validate().expect("agent auth form to validate");
        let config = details
            .saved
            .ssh_tunnel
            .expect("validated details to include ssh tunnel");

        assert_eq!(config.auth_method, SshAuthMethod::Agent);
        assert!(config.private_key_path.is_empty());
    }

    #[test]
    fn ssh_agent_auth_does_not_persist_save_secret() {
        let form = ConnectionForm {
            name: "Remote".to_string(),
            username: "postgres".to_string(),
            ssh_enabled: true,
            ssh_host: "bastion.example.com".to_string(),
            ssh_username: "anil".to_string(),
            ssh_auth_method: SshAuthMethod::Agent,
            ssh_save_secret: true,
            ..ConnectionForm::default()
        };

        let details = form.validate().expect("agent auth form to validate");
        let config = details
            .saved
            .ssh_tunnel
            .expect("validated details to include ssh tunnel");

        assert!(!config.save_secret);
    }

    #[test]
    fn ssh_agent_auth_does_not_persist_private_key_path() {
        let form = ConnectionForm {
            name: "Remote".to_string(),
            username: "postgres".to_string(),
            ssh_enabled: true,
            ssh_host: "bastion.example.com".to_string(),
            ssh_username: "anil".to_string(),
            ssh_auth_method: SshAuthMethod::Agent,
            ssh_private_key_path: "/home/anil/.ssh/id_ed25519".to_string(),
            ..ConnectionForm::default()
        };

        let details = form.validate().expect("agent auth form to validate");
        let config = details
            .saved
            .ssh_tunnel
            .expect("validated details to include ssh tunnel");

        assert!(config.private_key_path.is_empty());
    }

    #[test]
    fn connection_form_copies_ssh_agent_method_from_saved_connection() {
        let connection = SavedConnection {
            ssh_tunnel: Some(SshTunnelConfig {
                auth_method: SshAuthMethod::Agent,
                ..private_key_tunnel_config()
            }),
            ..saved_connection()
        };

        let form = ConnectionForm::from_saved(&connection);

        assert_eq!(form.ssh_auth_method, SshAuthMethod::Agent);
    }

    fn saved_connection() -> SavedConnection {
        SavedConnection {
            id: "pg-1".to_string(),
            name: "Local".to_string(),
            host: "localhost".to_string(),
            port: 5432,
            database: "postgres".to_string(),
            username: "anil".to_string(),
            save_password: false,
            ssh_tunnel: None,
        }
    }

    fn private_key_tunnel_config() -> SshTunnelConfig {
        SshTunnelConfig {
            host: "bastion.example.com".to_string(),
            port: 22,
            username: "anil".to_string(),
            auth_method: SshAuthMethod::PrivateKey,
            private_key_path: "/home/anil/.ssh/id_ed25519".to_string(),
            save_secret: true,
            host_key_fingerprint: Some("SHA256:abc".to_string()),
        }
    }
}
