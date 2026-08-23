use serde::{Deserialize, Serialize};

use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::query_result::DEFAULT_QUERY_RESULT_ROW_LIMIT;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    pub connection_id: String,
    pub database: String,
    pub active_tab: Option<SavedSessionTabId>,
    pub tabs: Vec<SavedSessionTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SavedSessionTab {
    Query {
        id: u64,
        sql: String,
        #[serde(default = "default_query_row_limit")]
        row_limit: usize,
    },
    Browse {
        id: u64,
        object: SavedSessionObject,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "id")]
pub enum SavedSessionTabId {
    Query(u64),
    Browse(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSessionObject {
    pub schema: String,
    pub name: String,
    pub kind: SavedSessionObjectKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedSessionObjectKind {
    Table,
    View,
}

impl SavedSessionTab {
    pub fn id(&self) -> SavedSessionTabId {
        match self {
            Self::Query { id, .. } => SavedSessionTabId::Query(*id),
            Self::Browse { id, .. } => SavedSessionTabId::Browse(*id),
        }
    }
}

impl SavedSessionObject {
    pub fn from_database_object(object: &DatabaseObject) -> Self {
        Self {
            schema: object.schema.clone(),
            name: object.name.clone(),
            kind: SavedSessionObjectKind::from_database_object_kind(&object.kind),
        }
    }

    pub fn to_database_object(&self) -> DatabaseObject {
        DatabaseObject {
            schema: self.schema.clone(),
            name: self.name.clone(),
            kind: self.kind.to_database_object_kind(),
        }
    }
}

impl SavedSessionObjectKind {
    fn from_database_object_kind(kind: &DatabaseObjectKind) -> Self {
        match kind {
            DatabaseObjectKind::Table => Self::Table,
            DatabaseObjectKind::View => Self::View,
        }
    }

    fn to_database_object_kind(self) -> DatabaseObjectKind {
        match self {
            Self::Table => DatabaseObjectKind::Table,
            Self::View => DatabaseObjectKind::View,
        }
    }
}

fn default_query_row_limit() -> usize {
    DEFAULT_QUERY_RESULT_ROW_LIMIT
}
