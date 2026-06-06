use crate::models::database_object::DatabaseObject;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureActionTarget {
    pub table: DatabaseObject,
    pub kind: StructureActionKind,
    pub name: String,
    pub editable: bool,
}

impl StructureActionTarget {
    pub fn new(
        table: DatabaseObject,
        kind: StructureActionKind,
        name: impl Into<String>,
        editable: bool,
    ) -> Self {
        Self {
            table,
            kind,
            name: name.into(),
            editable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureActionKind {
    Column,
    Index,
    Constraint,
    ForeignKey,
    Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureDropMode {
    Restrict,
    Cascade,
}
