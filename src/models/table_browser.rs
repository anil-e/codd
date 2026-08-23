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
    pub is_array: bool,
    pub is_range: bool,
    pub is_nullable: bool,
    pub is_primary_key: bool,
    pub has_default: bool,
    pub is_identity: bool,
    pub is_generated: bool,
    pub ordinal_position: i32,
}

impl TableColumn {
    pub fn is_editable_value_type(&self) -> bool {
        self.type_group != ColumnTypeGroup::Binary
    }

    pub fn is_insertable(&self) -> bool {
        !self.is_identity && !self.is_generated && self.is_editable_value_type()
    }

    pub fn is_required_for_insert(&self) -> bool {
        !self.is_nullable && !self.has_default && !self.is_identity && !self.is_generated
    }

    pub fn uses_text_display(&self) -> bool {
        self.is_array
            || self.is_range
            || matches!(self.type_group, ColumnTypeGroup::Json)
            || matches!(
                self.type_name.to_ascii_lowercase().as_str(),
                "numeric" | "decimal" | "money" | "inet" | "cidr" | "macaddr" | "macaddr8"
            )
            || self.type_name.eq_ignore_ascii_case("uuid")
            || !self.enum_values.is_empty()
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
pub enum TableInsertValue {
    Default,
    Null,
    Value(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSort {
    pub column_name: String,
    pub direction: SortDirection,
}

impl TableSort {
    pub fn new(column_name: String, direction: SortDirection) -> Self {
        Self {
            column_name,
            direction,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Ascending => "ASC",
            Self::Descending => "DESC",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableFilter {
    Column {
        column_name: String,
        operator: FilterOperator,
        value: Option<String>,
    },
    CustomSql {
        expression: String,
    },
}

impl TableFilter {
    pub fn column(column_name: String, operator: FilterOperator, value: Option<String>) -> Self {
        Self::Column {
            column_name,
            operator,
            value,
        }
    }

    pub fn custom_sql(expression: impl Into<String>) -> Self {
        Self::CustomSql {
            expression: expression.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    Equal,
    NotEqual,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Like,
    ILike,
    IsNull,
    IsNotNull,
}

impl FilterOperator {
    pub fn label(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::GreaterThan => ">",
            Self::GreaterThanOrEqual => ">=",
            Self::LessThan => "<",
            Self::LessThanOrEqual => "<=",
            Self::Like => "LIKE",
            Self::ILike => "ILIKE",
            Self::IsNull => "IS NULL",
            Self::IsNotNull => "IS NOT NULL",
        }
    }

    pub fn needs_value(self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }

    pub fn for_column(column: &TableColumn) -> &'static [Self] {
        match column.type_group {
            ColumnTypeGroup::Boolean => {
                &[Self::Equal, Self::NotEqual, Self::IsNull, Self::IsNotNull]
            }
            ColumnTypeGroup::DateTime | ColumnTypeGroup::Numeric => &[
                Self::Equal,
                Self::NotEqual,
                Self::GreaterThan,
                Self::GreaterThanOrEqual,
                Self::LessThan,
                Self::LessThanOrEqual,
                Self::IsNull,
                Self::IsNotNull,
            ],
            ColumnTypeGroup::Text => &[
                Self::Equal,
                Self::NotEqual,
                Self::Like,
                Self::ILike,
                Self::IsNull,
                Self::IsNotNull,
            ],
            ColumnTypeGroup::Binary | ColumnTypeGroup::Json => &[Self::IsNull, Self::IsNotNull],
            ColumnTypeGroup::Other if column.uses_text_display() => &[
                Self::Equal,
                Self::NotEqual,
                Self::Like,
                Self::ILike,
                Self::IsNull,
                Self::IsNotNull,
            ],
            ColumnTypeGroup::Other => &[Self::Equal, Self::NotEqual, Self::IsNull, Self::IsNotNull],
        }
    }

    pub fn is_supported_for(self, column: &TableColumn) -> bool {
        Self::for_column(column).contains(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::{ColumnTypeGroup, TableColumn};

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

    #[test]
    fn uses_text_display_for_types_that_need_stable_postgres_formatting() {
        assert!(column("numeric").uses_text_display());
        assert!(column("decimal").uses_text_display());
        assert!(column("money").uses_text_display());
        assert!(column("uuid").uses_text_display());
        assert!(column("inet").uses_text_display());
        assert!(column("cidr").uses_text_display());
        assert!(column("macaddr").uses_text_display());
        assert!(column("macaddr8").uses_text_display());
    }

    #[test]
    fn uses_text_display_for_postgres_special_types() {
        let mut column = column("jsonb");
        column.type_group = ColumnTypeGroup::Json;

        assert!(column.uses_text_display());
    }

    #[test]
    fn keeps_native_display_for_datetime_types() {
        assert!(!column("date").uses_text_display());
        assert!(!column("timetz").uses_text_display());
    }

    #[test]
    fn uses_text_display_for_arrays_and_ranges() {
        let mut array = column("_int4");
        array.is_array = true;
        let mut range = column("daterange");
        range.is_range = true;

        assert!(array.uses_text_display());
        assert!(range.uses_text_display());
    }

    #[test]
    fn keeps_native_display_for_unrecognized_postgres_types() {
        assert!(!column("custom_type").uses_text_display());
    }

    #[test]
    fn keeps_native_display_for_basic_numeric_types() {
        assert!(!column("int8").uses_text_display());
        assert!(!column("float8").uses_text_display());
    }

    fn column(type_name: &str) -> TableColumn {
        TableColumn {
            name: "value".to_string(),
            display_type: type_name.to_string(),
            type_name: type_name.to_string(),
            enum_values: Vec::new(),
            type_group: ColumnTypeGroup::from_postgres_type(type_name),
            is_array: false,
            is_range: false,
            is_nullable: false,
            is_primary_key: false,
            has_default: false,
            is_identity: false,
            is_generated: false,
            ordinal_position: 1,
        }
    }
}
