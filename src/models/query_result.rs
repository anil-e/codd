pub const DEFAULT_QUERY_RESULT_ROW_LIMIT: usize = 1_000;
pub const MIN_QUERY_RESULT_ROW_LIMIT: usize = 1;
pub const MAX_QUERY_RESULT_ROW_LIMIT: usize = 50_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_limit: Option<usize>,
    pub row_limit_reached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryExecutionResult {
    Rows(QueryResult),
    AffectedRows(u64),
}
