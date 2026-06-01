use std::fs;
use std::io;
use std::path::PathBuf;

use crate::models::query_result::{MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT};
use crate::models::session::{SavedSession, SavedSessionTab};

const MAX_TABS_PER_SESSION: usize = 32;
const MAX_QUERY_SQL_BYTES: usize = 256 * 1024;

pub fn load(connection_id: &str, database: &str) -> Option<SavedSession> {
    let content = fs::read_to_string(session_path(connection_id, database)).ok()?;
    let mut session = parse_session(&content).ok()?;

    if session.connection_id != connection_id || session.database != database {
        return None;
    }

    prune_session(&mut session);

    is_meaningful_session(&session).then_some(session)
}

pub fn save(session: &SavedSession) -> io::Result<()> {
    if session.connection_id.is_empty() || session.database.is_empty() {
        return Ok(());
    }

    let mut session = session.clone();
    prune_session(&mut session);

    let path = session_path(&session.connection_id, &session.database);

    if !is_meaningful_session(&session) {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(&session)?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, content)?;
    fs::rename(temp_path, path)
}

fn parse_session(content: &str) -> io::Result<SavedSession> {
    serde_json::from_str(content).map_err(io::Error::other)
}

fn prune_session(session: &mut SavedSession) {
    session.tabs.retain(valid_tab);
    session.tabs.truncate(MAX_TABS_PER_SESSION);

    for tab in &mut session.tabs {
        if let SavedSessionTab::Query { sql, row_limit, .. } = tab {
            *sql = trimmed_sql(sql);
            *row_limit = (*row_limit).clamp(MIN_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT);
        }
    }

    if let Some(active_tab) = session.active_tab
        && !session.tabs.iter().any(|tab| tab.id() == active_tab)
    {
        session.active_tab = session.tabs.last().map(SavedSessionTab::id);
    }
}

fn valid_tab(tab: &SavedSessionTab) -> bool {
    match tab {
        SavedSessionTab::Query { .. } => true,
        SavedSessionTab::Browse { object, .. } => {
            !object.schema.trim().is_empty() && !object.name.trim().is_empty()
        }
    }
}

fn is_meaningful_session(session: &SavedSession) -> bool {
    session.tabs.iter().any(|tab| match tab {
        SavedSessionTab::Query { sql, .. } => !sql.trim().is_empty(),
        SavedSessionTab::Browse { .. } => true,
    })
}

fn trimmed_sql(sql: &str) -> String {
    let sql = sql.trim();
    if sql.len() <= MAX_QUERY_SQL_BYTES {
        return sql.to_string();
    }

    sql.char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= MAX_QUERY_SQL_BYTES)
        .map(|(_, character)| character)
        .collect::<String>()
        .trim()
        .to_string()
}

fn session_path(connection_id: &str, database: &str) -> PathBuf {
    super::connection_store::config_dir()
        .join("sessions")
        .join(safe_path_segment(connection_id))
        .join(format!("{}.json", safe_path_segment(database)))
}

fn safe_path_segment(value: &str) -> String {
    let mut segment = String::new();

    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => {
                segment.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(&mut segment, "%{byte:02x}");
            }
        }
    }

    segment
}

#[cfg(test)]
mod tests {
    use super::{parse_session, safe_path_segment, trimmed_sql};
    use crate::models::query_result::{DEFAULT_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT};
    use crate::models::session::{SavedSessionTab, SavedSessionTabId};

    #[test]
    fn trims_large_sql_without_breaking_utf8() {
        let sql = "ä".repeat(200_000);
        let trimmed = trimmed_sql(&sql);

        assert!(trimmed.len() <= super::MAX_QUERY_SQL_BYTES);
        assert!(trimmed.is_char_boundary(trimmed.len()));
    }

    #[test]
    fn parses_legacy_query_tab_without_row_limit() {
        let session = parse_session(
            r#"{
                "connection_id": "pg-1",
                "database": "postgres",
                "active_tab": { "type": "Query", "id": 1 },
                "tabs": [
                    { "type": "Query", "id": 1, "sql": "select 1;" }
                ]
            }"#,
        )
        .expect("session to parse");

        assert_eq!(session.tabs.len(), 1);
        assert!(matches!(
            &session.tabs[0],
            SavedSessionTab::Query { row_limit, .. } if *row_limit == DEFAULT_QUERY_RESULT_ROW_LIMIT
        ));
    }

    #[test]
    fn keeps_empty_query_tabs_when_session_has_content() {
        let mut session = parse_session(
            r#"{
                "connection_id": "pg-1",
                "database": "postgres",
                "active_tab": { "type": "Query", "id": 99 },
                "tabs": [
                    { "type": "Query", "id": 1, "sql": "" },
                    { "type": "Query", "id": 2, "sql": "select 1;", "row_limit": 999999999 },
                    {
                        "type": "Browse",
                        "id": 3,
                        "object": { "schema": "", "name": "users", "kind": "table" }
                    }
                ]
            }"#,
        )
        .expect("session to parse");

        super::prune_session(&mut session);

        assert_eq!(session.tabs.len(), 2);
        assert_eq!(session.active_tab, Some(SavedSessionTabId::Query(2)));
        assert!(matches!(
            &session.tabs[1],
            SavedSessionTab::Query { row_limit, .. } if *row_limit == MAX_QUERY_RESULT_ROW_LIMIT
        ));
    }

    #[test]
    fn treats_only_empty_query_tabs_as_not_meaningful() {
        let mut session = parse_session(
            r#"{
                "connection_id": "pg-1",
                "database": "postgres",
                "active_tab": { "type": "Query", "id": 1 },
                "tabs": [
                    { "type": "Query", "id": 1, "sql": "" }
                ]
            }"#,
        )
        .expect("session to parse");

        super::prune_session(&mut session);

        assert!(!super::is_meaningful_session(&session));
    }

    #[test]
    fn encodes_path_segments() {
        assert_eq!(safe_path_segment("pg-1"), "pg-1");
        assert_eq!(safe_path_segment("prod/db"), "prod%2fdb");
    }
}
