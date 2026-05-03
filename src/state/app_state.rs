use crate::models::connection::SavedConnection;
use crate::models::database_object::DatabaseObject;
use crate::models::query_result::QueryResult;

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub connections: Vec<SavedConnection>,
    pub active_connection: Option<SavedConnection>,
    pub objects: Vec<DatabaseObject>,
    pub query_result: Option<QueryResult>,
}
