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
    pub fn select_limit_query(&self) -> String {
        format!("SELECT * FROM {} LIMIT 100;", self.qualified_name())
    }

    fn qualified_name(&self) -> String {
        format!(
            "{}.{}",
            quote_identifier(&self.schema),
            quote_identifier(&self.name)
        )
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
