use crate::models::connection::SavedConnection;
use crate::models::database_object::DatabaseObject;

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub connections: Vec<SavedConnection>,
    pub active_connection: Option<SavedConnection>,
    pub objects: Vec<DatabaseObject>,
}
