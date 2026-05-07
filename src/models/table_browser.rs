use crate::models::database_object::DatabaseObject;

pub const DEFAULT_PAGE_SIZE: u32 = 100;
pub const PAGE_SIZE_OPTIONS: &[u32] = &[50, 100, 250, 500];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnTypeGroup {
    Boolean,
    Binary,
    DateTime,
    Json,
    Numeric,
    Text,
    Other,
}

impl ColumnTypeGroup {
    pub fn from_postgres_type(type_name: &str) -> Self {
        match type_name.to_ascii_lowercase().as_str() {
            "bool" | "boolean" => Self::Boolean,
            "bytea" => Self::Binary,
            "date" | "time" | "timetz" | "timestamp" | "timestamptz" => Self::DateTime,
            "json" | "jsonb" => Self::Json,
            "int2" | "int4" | "int8" | "float4" | "float8" | "numeric" | "money" | "oid" => {
                Self::Numeric
            }
            "bpchar" | "char" | "citext" | "name" | "text" | "varchar" => Self::Text,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableColumn {
    pub name: String,
    pub display_type: String,
    pub type_name: String,
    pub enum_values: Vec<String>,
    pub type_group: ColumnTypeGroup,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub ordinal_position: i32,
}

impl TableColumn {
    pub fn is_editable_value_type(&self) -> bool {
        self.type_group != ColumnTypeGroup::Binary
    }

    pub fn uses_text_display(&self) -> bool {
        self.type_name.eq_ignore_ascii_case("uuid") || !self.enum_values.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    pub value: String,
    pub is_null: bool,
}

impl TableCell {
    pub fn null() -> Self {
        Self {
            value: "NULL".to_string(),
            is_null: true,
        }
    }

    pub fn new(value: String) -> Self {
        Self {
            value,
            is_null: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePage {
    pub object: DatabaseObject,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<Vec<TableCell>>,
    pub offset: u32,
    pub page_size: u32,
    pub has_next_page: bool,
}

#[cfg(test)]
mod tests {
    use super::ColumnTypeGroup;

    #[test]
    fn groups_common_postgres_types() {
        assert_eq!(
            ColumnTypeGroup::from_postgres_type("BOOL"),
            ColumnTypeGroup::Boolean
        );
        assert_eq!(
            ColumnTypeGroup::from_postgres_type("int8"),
            ColumnTypeGroup::Numeric
        );
        assert_eq!(
            ColumnTypeGroup::from_postgres_type("VARCHAR"),
            ColumnTypeGroup::Text
        );
        assert_eq!(
            ColumnTypeGroup::from_postgres_type("timestamptz"),
            ColumnTypeGroup::DateTime
        );
        assert_eq!(
            ColumnTypeGroup::from_postgres_type("jsonb"),
            ColumnTypeGroup::Json
        );
        assert_eq!(
            ColumnTypeGroup::from_postgres_type("uuid"),
            ColumnTypeGroup::Other
        );
    }
}
