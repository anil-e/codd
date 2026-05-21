use crate::models::database_object::DatabaseObject;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStructure {
    pub object: DatabaseObject,
    pub columns: Vec<TableStructureColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStructureColumn {
    pub name: String,
    pub data_type: String,
    pub type_name: String,
    pub is_nullable: bool,
    pub default_expression: Option<String>,
    pub is_primary_key: bool,
    pub identity: Option<TableColumnIdentity>,
    pub generated: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableColumnIdentity {
    Always,
    ByDefault,
}

impl TableColumnIdentity {
    pub fn from_postgres_identity(value: &str) -> Option<Self> {
        match value {
            "a" => Some(Self::Always),
            "d" => Some(Self::ByDefault),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::ByDefault => "By default",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TableColumnIdentity;

    #[test]
    fn maps_postgres_identity_codes() {
        assert_eq!(
            TableColumnIdentity::from_postgres_identity("a"),
            Some(TableColumnIdentity::Always)
        );
        assert_eq!(
            TableColumnIdentity::from_postgres_identity("d"),
            Some(TableColumnIdentity::ByDefault)
        );
        assert_eq!(TableColumnIdentity::from_postgres_identity(""), None);
    }
}
