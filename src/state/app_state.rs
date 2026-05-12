use crate::models::connection::SavedConnection;
use crate::models::database_object::DatabaseObject;

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub connections: Vec<SavedConnection>,
    pub active_connection: Option<SavedConnection>,
    pub active_database: Option<String>,
    pub available_databases: Vec<String>,
    pub objects: Vec<DatabaseObject>,
}
