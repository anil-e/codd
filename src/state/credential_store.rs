use std::collections::HashMap;

use crate::config::APP_ID;
use crate::models::connection::SavedConnection;

const SECRET_TYPE: &str = "postgres-password";

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
    if connection_id.is_empty() {
        return Ok(None);
    }

    let keyring = unlocked_keyring().await?;
    let attributes = password_attributes(connection_id);
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
    let keyring = unlocked_keyring().await?;
    let attributes = password_attributes(&connection.id);
    let label = format!("Codd password for {}", connection.name);

    keyring
        .create_item(&label, &attributes, password.as_bytes(), true)
        .await?;

    Ok(())
}

pub async fn delete_password(connection_id: &str) -> Result<(), CredentialError> {
    if connection_id.is_empty() {
        return Ok(());
    }

    let keyring = unlocked_keyring().await?;
    let attributes = password_attributes(connection_id);
    keyring.delete(&attributes).await?;

    Ok(())
}

pub async fn has_password(connection_id: &str) -> Result<bool, CredentialError> {
    if connection_id.is_empty() {
        return Ok(false);
    }

    let keyring = unlocked_keyring().await?;
    let attributes = password_attributes(connection_id);

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

fn password_attributes(connection_id: &str) -> HashMap<&str, &str> {
    HashMap::from([
        ("application", APP_ID),
        ("type", SECRET_TYPE),
        ("connection_id", connection_id),
    ])
}

#[cfg(test)]
mod tests {
    use super::{SECRET_TYPE, delete_password, has_password, load_password, password_attributes};
    use crate::config::APP_ID;

    #[test]
    fn password_attributes_identify_connection_passwords() {
        let attributes = password_attributes("pg-1");

        assert_eq!(attributes.get("application"), Some(&APP_ID));
        assert_eq!(attributes.get("type"), Some(&SECRET_TYPE));
        assert_eq!(attributes.get("connection_id"), Some(&"pg-1"));
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
