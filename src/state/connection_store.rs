use crate::models::connection::SavedConnection;
use std::fs;
use std::io;
use std::path::PathBuf;

pub fn load_connections() -> Vec<SavedConnection> {
    let path = connections_path();
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut connections: Vec<SavedConnection> = serde_json::from_str(&content).unwrap_or_default();
    deduplicate_connections(&mut connections);
    connections
}

pub fn save_connection(connection: &SavedConnection) -> io::Result<Vec<SavedConnection>> {
    update_connections(|connections| {
        if let Some(existing) = connections.iter_mut().find(|item| item.id == connection.id) {
            *existing = connection.clone();
        } else {
            connections.push(connection.clone());
        }
    })
}

pub fn rename_connection(connection_id: &str, new_name: &str) -> io::Result<Vec<SavedConnection>> {
    update_connections(|connections| {
        if let Some(existing) = connections.iter_mut().find(|item| item.id == connection_id) {
            existing.name = new_name.to_string();
        }
    })
}

pub fn set_save_password(
    connection_id: &str,
    save_password: bool,
) -> io::Result<Vec<SavedConnection>> {
    update_connections(|connections| {
        if let Some(existing) = connections.iter_mut().find(|item| item.id == connection_id) {
            existing.save_password = save_password;
        }
    })
}

pub fn remove_connection(connection_id: &str) -> io::Result<Vec<SavedConnection>> {
    update_connections(|connections| {
        connections.retain(|item| item.id != connection_id);
    })
}

fn update_connections(
    mut update: impl FnMut(&mut Vec<SavedConnection>),
) -> io::Result<Vec<SavedConnection>> {
    let mut connections = load_connections();
    update(&mut connections);
    deduplicate_connections(&mut connections);
    atomic_save_connections(&connections)?;
    Ok(connections)
}

fn deduplicate_connections(connections: &mut Vec<SavedConnection>) {
    let mut seen_ids = std::collections::HashSet::new();
    connections.retain(|connection| seen_ids.insert(connection.id.clone()));
}

fn atomic_save_connections(connections: &[SavedConnection]) -> io::Result<()> {
    let path = connections_path();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(connections)?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, content)?;
    fs::rename(temp_path, path)
}

fn connections_path() -> PathBuf {
    config_dir().join("connections.json")
}

pub(super) fn config_dir() -> PathBuf {
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(value).join("codd");
    }

    if let Some(value) = std::env::var_os("HOME") {
        return PathBuf::from(value).join(".config").join("codd");
    }

    PathBuf::from(".")
}
