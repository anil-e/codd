#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableScriptKind {
    Create,
    Select,
    Insert,
    Update,
    Delete,
}
