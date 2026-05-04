use crate::models::query_history::QueryHistoryEntry;
use chrono::Utc;
use std::fs;
use std::io;
use std::path::PathBuf;

const MAX_ENTRIES_PER_CONNECTION: usize = 100;
const MAX_TOTAL_ENTRIES: usize = 1_000;
const MAX_SQL_BYTES: usize = 64 * 1024;

pub fn load_for_connection(connection_id: &str) -> Vec<QueryHistoryEntry> {
    read_history()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.connection_id == connection_id)
        .collect()
}

pub fn record_query(connection_id: &str, sql: &str) -> io::Result<Vec<QueryHistoryEntry>> {
    let sql = trimmed_query(sql);
    if sql.is_empty() {
        return read_history_for_connection(connection_id);
    }

    let mut history = read_history()?;
    record_query_in_memory(&mut history, connection_id, sql, Utc::now().timestamp());
    atomic_save_history(&history)?;

    Ok(filter_connection_history(history, connection_id))
}

pub fn clear_connection(connection_id: &str) -> io::Result<Vec<QueryHistoryEntry>> {
    let mut history = read_history()?;
    history.retain(|entry| entry.connection_id != connection_id);
    atomic_save_history(&history)?;
    Ok(Vec::new())
}

fn read_history_for_connection(connection_id: &str) -> io::Result<Vec<QueryHistoryEntry>> {
    read_history().map(|history| filter_connection_history(history, connection_id))
}

fn filter_connection_history(
    history: Vec<QueryHistoryEntry>,
    connection_id: &str,
) -> Vec<QueryHistoryEntry> {
    history
        .into_iter()
        .filter(|entry| entry.connection_id == connection_id)
        .collect()
}

fn record_query_in_memory(
    history: &mut Vec<QueryHistoryEntry>,
    connection_id: &str,
    sql: String,
    executed_at: i64,
) {
    history.retain(|entry| !(entry.connection_id == connection_id && entry.sql == sql));
    history.insert(
        0,
        QueryHistoryEntry {
            connection_id: connection_id.to_string(),
            sql,
            executed_at,
        },
    );
    prune_history(history);
}

fn read_history() -> io::Result<Vec<QueryHistoryEntry>> {
    let content = match fs::read_to_string(history_path()) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    parse_history(&content)
}

fn parse_history(content: &str) -> io::Result<Vec<QueryHistoryEntry>> {
    let mut history: Vec<QueryHistoryEntry> = serde_json::from_str(content)?;
    history.retain(|entry| !entry.connection_id.is_empty() && !entry.sql.trim().is_empty());
    history.sort_by(|a, b| b.executed_at.cmp(&a.executed_at));
    prune_history(&mut history);
    Ok(history)
}

fn prune_history(history: &mut Vec<QueryHistoryEntry>) {
    let mut per_connection = std::collections::HashMap::new();
    history.retain(|entry| {
        let count = per_connection
            .entry(entry.connection_id.clone())
            .and_modify(|count| *count += 1)
            .or_insert(1);

        *count <= MAX_ENTRIES_PER_CONNECTION
    });
    history.truncate(MAX_TOTAL_ENTRIES);
}

fn trimmed_query(sql: &str) -> String {
    let sql = sql.trim();
    if sql.len() <= MAX_SQL_BYTES {
        return sql.to_string();
    }

    sql.char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= MAX_SQL_BYTES)
        .map(|(_, character)| character)
        .collect::<String>()
        .trim()
        .to_string()
}

fn atomic_save_history(history: &[QueryHistoryEntry]) -> io::Result<()> {
    let path = history_path();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(history)?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, content)?;
    fs::rename(temp_path, path)
}

fn history_path() -> PathBuf {
    super::connection_store::config_dir().join("query-history.json")
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_ENTRIES_PER_CONNECTION, QueryHistoryEntry, parse_history, record_query_in_memory,
        trimmed_query,
    };

    #[test]
    fn trimmed_query_removes_outer_whitespace() {
        assert_eq!(trimmed_query("\n select 1; \n"), "select 1;");
    }

    #[test]
    fn trimmed_query_keeps_utf8_boundaries() {
        let sql = format!("select '{}';", "ä".repeat(40_000));
        assert!(trimmed_query(&sql).is_char_boundary(trimmed_query(&sql).len()));
    }

    #[test]
    fn trimmed_query_respects_byte_limit() {
        let sql = "ä".repeat(40_000);
        assert!(trimmed_query(&sql).len() <= super::MAX_SQL_BYTES);
    }

    #[test]
    fn recording_query_moves_duplicates_to_the_top() {
        let mut history = vec![
            entry("connection-a", "select 1;", 10),
            entry("connection-a", "select 2;", 20),
        ];

        record_query_in_memory(&mut history, "connection-a", "select 1;".to_string(), 30);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].sql, "select 1;");
        assert_eq!(history[0].executed_at, 30);
        assert_eq!(history[1].sql, "select 2;");
    }

    #[test]
    fn recording_query_keeps_separate_connection_histories() {
        let mut history = vec![entry("connection-b", "select 1;", 10)];

        record_query_in_memory(&mut history, "connection-a", "select 1;".to_string(), 20);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].connection_id, "connection-a");
        assert_eq!(history[1].connection_id, "connection-b");
    }

    #[test]
    fn recording_query_limits_entries_per_connection() {
        let mut history = (0..MAX_ENTRIES_PER_CONNECTION)
            .map(|index| entry("connection-a", &format!("select {index};"), index as i64))
            .collect();

        record_query_in_memory(
            &mut history,
            "connection-a",
            "select newest;".to_string(),
            1_000,
        );

        let connection_entries = history
            .iter()
            .filter(|entry| entry.connection_id == "connection-a")
            .count();
        assert_eq!(connection_entries, MAX_ENTRIES_PER_CONNECTION);
        assert_eq!(history[0].sql, "select newest;");
    }

    fn entry(connection_id: &str, sql: &str, executed_at: i64) -> QueryHistoryEntry {
        QueryHistoryEntry {
            connection_id: connection_id.to_string(),
            sql: sql.to_string(),
            executed_at,
        }
    }

    #[test]
    fn parse_history_rejects_invalid_json() {
        assert!(parse_history("not json").is_err());
    }

    #[test]
    fn parse_history_filters_invalid_entries_and_sorts_newest_first() {
        let history = parse_history(
            r#"
            [
              { "connection_id": "connection-a", "sql": "select older;", "executed_at": 10 },
              { "connection_id": "", "sql": "select invalid;", "executed_at": 30 },
              { "connection_id": "connection-a", "sql": "   ", "executed_at": 40 },
              { "connection_id": "connection-a", "sql": "select newer;", "executed_at": 20 }
            ]
            "#,
        )
        .expect("valid history json to parse");

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].sql, "select newer;");
        assert_eq!(history[1].sql, "select older;");
    }
}
