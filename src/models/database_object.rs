#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseObjectKind {
    Table,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseObject {
    pub schema: String,
    pub name: String,
    pub kind: DatabaseObjectKind,
}

impl DatabaseObject {
    pub fn qualified_name(&self) -> String {
        format!(
            "{}.{}",
            quote_identifier(&self.schema),
            quote_identifier(&self.name)
        )
    }
}

pub fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{DatabaseObject, DatabaseObjectKind, quote_identifier};

    #[test]
    fn quotes_postgres_identifiers() {
        assert_eq!(quote_identifier("public"), "\"public\"");
        assert_eq!(quote_identifier("user data"), "\"user data\"");
        assert_eq!(quote_identifier("weird\"name"), "\"weird\"\"name\"");
    }

    #[test]
    fn builds_qualified_names() {
        let object = DatabaseObject {
            schema: "analytics".to_string(),
            name: "page views".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(object.qualified_name(), "\"analytics\".\"page views\"");
    }
}
