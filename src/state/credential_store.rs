use std::collections::HashMap;

use crate::config::APP_ID;
use crate::models::connection::SavedConnection;

const POSTGRES_PASSWORD_SECRET_TYPE: &str = "postgres-password";
const SSH_PASSWORD_SECRET_TYPE: &str = "ssh-password";
const SSH_KEY_PASSPHRASE_SECRET_TYPE: &str = "ssh-key-passphrase";

#[derive(Debug)]
pub enum CredentialError {
    Keyring(oo7::Error),
    InvalidSecret(std::string::FromUtf8Error),
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keyring(error) => write!(formatter, "{error}"),
            Self::InvalidSecret(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CredentialError {}

impl From<oo7::Error> for CredentialError {
    fn from(error: oo7::Error) -> Self {
        Self::Keyring(error)
    }
}

impl From<std::string::FromUtf8Error> for CredentialError {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Self::InvalidSecret(error)
    }
}

pub async fn load_password(connection_id: &str) -> Result<Option<String>, CredentialError> {
    load_secret(connection_id, POSTGRES_PASSWORD_SECRET_TYPE).await
}

pub async fn load_ssh_password(connection_id: &str) -> Result<Option<String>, CredentialError> {
    load_secret(connection_id, SSH_PASSWORD_SECRET_TYPE).await
}

pub async fn load_ssh_key_passphrase(
    connection_id: &str,
) -> Result<Option<String>, CredentialError> {
    load_secret(connection_id, SSH_KEY_PASSPHRASE_SECRET_TYPE).await
}

async fn load_secret(
    connection_id: &str,
    secret_type: &'static str,
) -> Result<Option<String>, CredentialError> {
    if connection_id.is_empty() {
        return Ok(None);
    }

    let keyring = unlocked_keyring().await?;
    let attributes = secret_attributes(connection_id, secret_type);
    let items = keyring.search_items(&attributes).await?;

    match items.first() {
        Some(item) => Ok(Some(String::from_utf8(item.secret().await?.to_vec())?)),
        None => Ok(None),
    }
}

pub async fn store_password(
    connection: &SavedConnection,
    password: &str,
) -> Result<(), CredentialError> {
    store_secret(
        &connection.id,
        POSTGRES_PASSWORD_SECRET_TYPE,
        &format!("Codd password for {}", connection.name),
        password,
    )
    .await
}

pub async fn store_ssh_password(
    connection: &SavedConnection,
    password: &str,
) -> Result<(), CredentialError> {
    store_secret(
        &connection.id,
        SSH_PASSWORD_SECRET_TYPE,
        &format!("Codd SSH password for {}", connection.name),
        password,
    )
    .await
}

pub async fn store_ssh_key_passphrase(
    connection: &SavedConnection,
    passphrase: &str,
) -> Result<(), CredentialError> {
    store_secret(
        &connection.id,
        SSH_KEY_PASSPHRASE_SECRET_TYPE,
        &format!("Codd SSH key passphrase for {}", connection.name),
        passphrase,
    )
    .await
}

async fn store_secret(
    connection_id: &str,
    secret_type: &'static str,
    label: &str,
    secret: &str,
) -> Result<(), CredentialError> {
    let keyring = unlocked_keyring().await?;
    let attributes = secret_attributes(connection_id, secret_type);

    keyring
        .create_item(label, &attributes, secret.as_bytes(), true)
        .await?;

    Ok(())
}

pub async fn delete_password(connection_id: &str) -> Result<(), CredentialError> {
    delete_secret(connection_id, POSTGRES_PASSWORD_SECRET_TYPE).await
}

pub async fn delete_ssh_password(connection_id: &str) -> Result<(), CredentialError> {
    delete_secret(connection_id, SSH_PASSWORD_SECRET_TYPE).await
}

pub async fn delete_ssh_key_passphrase(connection_id: &str) -> Result<(), CredentialError> {
    delete_secret(connection_id, SSH_KEY_PASSPHRASE_SECRET_TYPE).await
}

async fn delete_secret(
    connection_id: &str,
    secret_type: &'static str,
) -> Result<(), CredentialError> {
    if connection_id.is_empty() {
        return Ok(());
    }

    let keyring = unlocked_keyring().await?;
    let attributes = secret_attributes(connection_id, secret_type);
    keyring.delete(&attributes).await?;

    Ok(())
}

pub async fn has_password(connection_id: &str) -> Result<bool, CredentialError> {
    has_secret(connection_id, POSTGRES_PASSWORD_SECRET_TYPE).await
}

async fn has_secret(
    connection_id: &str,
    secret_type: &'static str,
) -> Result<bool, CredentialError> {
    if connection_id.is_empty() {
        return Ok(false);
    }

    let keyring = unlocked_keyring().await?;
    let attributes = secret_attributes(connection_id, secret_type);

    Ok(!keyring.search_items(&attributes).await?.is_empty())
}

pub async fn is_available() -> Result<(), CredentialError> {
    let _ = unlocked_keyring().await?;

    Ok(())
}

async fn unlocked_keyring() -> Result<oo7::Keyring, CredentialError> {
    let keyring = oo7::Keyring::new().await?;
    keyring.unlock().await?;

    Ok(keyring)
}

fn secret_attributes<'a>(
    connection_id: &'a str,
    secret_type: &'static str,
) -> HashMap<&'static str, &'a str> {
    HashMap::from([
        ("application", APP_ID),
        ("type", secret_type),
        ("connection_id", connection_id),
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        POSTGRES_PASSWORD_SECRET_TYPE, SSH_KEY_PASSPHRASE_SECRET_TYPE, SSH_PASSWORD_SECRET_TYPE,
        delete_password, has_password, load_password, secret_attributes,
    };
    use crate::config::APP_ID;

    #[test]
    fn password_attributes_identify_connection_passwords() {
        let attributes = secret_attributes("pg-1", POSTGRES_PASSWORD_SECRET_TYPE);

        assert_eq!(attributes.get("application"), Some(&APP_ID));
        assert_eq!(attributes.get("type"), Some(&POSTGRES_PASSWORD_SECRET_TYPE));
        assert_eq!(attributes.get("connection_id"), Some(&"pg-1"));
    }

    #[test]
    fn ssh_secret_attributes_use_distinct_types() {
        let ssh_password = secret_attributes("pg-1", SSH_PASSWORD_SECRET_TYPE);
        let ssh_passphrase = secret_attributes("pg-1", SSH_KEY_PASSPHRASE_SECRET_TYPE);

        assert_eq!(ssh_password.get("type"), Some(&SSH_PASSWORD_SECRET_TYPE));
        assert_eq!(
            ssh_passphrase.get("type"),
            Some(&SSH_KEY_PASSPHRASE_SECRET_TYPE)
        );
    }

    #[tokio::test]
    async fn empty_connection_id_is_ignored() {
        assert!(
            load_password("")
                .await
                .expect("empty lookup to succeed")
                .is_none()
        );
        assert!(!has_password("").await.expect("empty check to succeed"));
        delete_password("").await.expect("empty delete to succeed");
    }
}
