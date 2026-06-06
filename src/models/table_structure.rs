use crate::models::database_object::DatabaseObject;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableStructure {
    pub object: DatabaseObject,
    pub columns: Vec<TableStructureColumn>,
    pub indexes: Vec<TableIndex>,
    pub constraints: Vec<TableConstraint>,
    pub foreign_keys: Vec<TableForeignKey>,
    pub triggers: Vec<TableTrigger>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableIndex {
    pub schema: String,
    pub name: String,
    pub method: String,
    pub definition: String,
    pub predicate: Option<String>,
    pub is_unique: bool,
    pub is_primary: bool,
    pub is_valid: bool,
    pub is_constraint_backed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableConstraint {
    pub name: String,
    pub kind: TableConstraintKind,
    pub definition: String,
    pub is_validated: bool,
    pub is_deferrable: bool,
    pub is_initially_deferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableConstraintKind {
    Check,
    ForeignKey,
    PrimaryKey,
    Unique,
    Exclusion,
    Other,
}

impl TableConstraintKind {
    pub fn from_postgres_code(value: &str) -> Self {
        match value {
            "c" => Self::Check,
            "f" => Self::ForeignKey,
            "p" => Self::PrimaryKey,
            "u" => Self::Unique,
            "x" => Self::Exclusion,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Check => "Check",
            Self::ForeignKey => "Foreign key",
            Self::PrimaryKey => "Primary key",
            Self::Unique => "Unique",
            Self::Exclusion => "Exclusion",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    pub on_update: ForeignKeyAction,
    pub on_delete: ForeignKeyAction,
    pub is_deferrable: bool,
    pub is_initially_deferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignKeyAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
    Other,
}

impl ForeignKeyAction {
    pub fn from_postgres_code(value: &str) -> Self {
        match value {
            "a" => Self::NoAction,
            "r" => Self::Restrict,
            "c" => Self::Cascade,
            "n" => Self::SetNull,
            "d" => Self::SetDefault,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NoAction => "No action",
            Self::Restrict => "Restrict",
            Self::Cascade => "Cascade",
            Self::SetNull => "Set null",
            Self::SetDefault => "Set default",
            Self::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableTrigger {
    pub name: String,
    pub definition: String,
    pub enabled: TriggerEnabledState,
    pub function_schema: String,
    pub function_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerEnabledState {
    Origin,
    Disabled,
    Always,
    Replica,
    Other,
}

impl TriggerEnabledState {
    pub fn from_postgres_code(value: &str) -> Self {
        match value {
            "O" => Self::Origin,
            "D" => Self::Disabled,
            "A" => Self::Always,
            "R" => Self::Replica,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Origin => "Enabled",
            Self::Disabled => "Disabled",
            Self::Always => "Always",
            Self::Replica => "Replica",
            Self::Other => "Other",
        }
    }
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
    use super::{ForeignKeyAction, TableColumnIdentity, TableConstraintKind, TriggerEnabledState};

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

    #[test]
    fn maps_constraint_codes() {
        assert_eq!(
            TableConstraintKind::from_postgres_code("p"),
            TableConstraintKind::PrimaryKey
        );
        assert_eq!(
            TableConstraintKind::from_postgres_code("f"),
            TableConstraintKind::ForeignKey
        );
        assert_eq!(
            TableConstraintKind::from_postgres_code("?"),
            TableConstraintKind::Other
        );
    }

    #[test]
    fn maps_foreign_key_action_codes() {
        assert_eq!(
            ForeignKeyAction::from_postgres_code("c"),
            ForeignKeyAction::Cascade
        );
        assert_eq!(
            ForeignKeyAction::from_postgres_code("n"),
            ForeignKeyAction::SetNull
        );
        assert_eq!(
            ForeignKeyAction::from_postgres_code("?"),
            ForeignKeyAction::Other
        );
    }

    #[test]
    fn maps_trigger_enabled_codes() {
        assert_eq!(
            TriggerEnabledState::from_postgres_code("O"),
            TriggerEnabledState::Origin
        );
        assert_eq!(
            TriggerEnabledState::from_postgres_code("D"),
            TriggerEnabledState::Disabled
        );
        assert_eq!(
            TriggerEnabledState::from_postgres_code("?"),
            TriggerEnabledState::Other
        );
    }
}
