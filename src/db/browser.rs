use crate::db::query;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind, quote_identifier};
use crate::models::table_browser::{
    ColumnTypeGroup, FilterOperator, TableCell, TableColumn, TableFilter, TablePage,
};
use sqlx::{Column, PgPool, Row, TypeInfo};

pub async fn load_table_page(
    pool: &PgPool,
    object: &DatabaseObject,
    offset: u32,
    page_size: u32,
    filters: &[TableFilter],
) -> Result<TablePage, sqlx::Error> {
    let columns = load_table_columns(pool, object).await?;
    let mut rows = load_page_rows(pool, object, &columns, offset, page_size, filters).await?;
    let has_next_page = rows.len() > page_size as usize;

    if has_next_page {
        rows.truncate(page_size as usize);
    }

    Ok(TablePage {
        object: object.clone(),
        columns,
        rows,
        offset,
        page_size,
        has_next_page,
    })
}

pub async fn load_table_columns(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableColumn>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, Option<Vec<String>>, bool, bool, i32)>(
        r"
        SELECT
            a.attname,
            format_type(a.atttypid, a.atttypmod) AS display_type,
            t.typname,
            array_agg(e.enumlabel ORDER BY e.enumsortorder)
                FILTER (WHERE e.enumlabel IS NOT NULL) AS enum_values,
            NOT a.attnotnull AS is_nullable,
            EXISTS (
                SELECT 1
                FROM pg_constraint con
                WHERE con.conrelid = c.oid
                  AND con.contype = 'p'
                  AND a.attnum = ANY(con.conkey)
            ) AS is_primary_key,
            a.attnum::int4 AS ordinal_position
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_attribute a ON a.attrelid = c.oid
        JOIN pg_type t ON t.oid = a.atttypid
        LEFT JOIN pg_enum e ON e.enumtypid = t.oid
        WHERE n.nspname = $1
          AND c.relname = $2
          AND a.attnum > 0
          AND NOT a.attisdropped
          AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
        GROUP BY
            c.oid,
            a.attname,
            a.atttypid,
            a.atttypmod,
            t.typname,
            a.attnotnull,
            a.attnum
        ORDER BY a.attnum
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                name,
                display_type,
                type_name,
                enum_values,
                is_nullable,
                is_primary_key,
                ordinal_position,
            )| {
                TableColumn {
                    name,
                    display_type,
                    enum_values: enum_values.unwrap_or_default(),
                    type_group: ColumnTypeGroup::from_postgres_type(&type_name),
                    type_name,
                    is_nullable,
                    is_primary_key,
                    ordinal_position,
                }
            },
        )
        .collect())
}

async fn load_page_rows(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
    filters: &[TableFilter],
) -> Result<Vec<Vec<TableCell>>, sqlx::Error> {
    let query = table_page_sql_with_filters(object, columns, offset, page_size, filters)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let mut sqlx_query = sqlx::query(&query.sql);

    for value in query.filter_values {
        sqlx_query = sqlx_query.bind(value);
    }

    let rows = sqlx_query.fetch_all(pool).await?;

    Ok(rows
        .iter()
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(index, column)| query::value_to_cell(row, index, column.type_info().name()))
                .collect()
        })
        .collect())
}

pub async fn update_table_cell(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    row: &[TableCell],
    column_index: usize,
    value: Option<String>,
) -> Result<TableCell, TableCellUpdateError> {
    validate_update_input(object, columns, row, column_index)?;

    let target_column = &columns[column_index];
    if value.is_none() && !target_column.is_nullable {
        return Err(TableCellUpdateError::NotNullable);
    }

    let sql = update_cell_sql(object, columns, column_index)?;
    let mut query = sqlx::query(&sql).bind(value);

    for (index, _) in primary_key_columns(columns) {
        let cell = &row[index];
        let value = (!cell.is_null).then(|| cell.value.clone());
        query = query.bind(value);
    }

    let row = query
        .fetch_one(pool)
        .await
        .map_err(TableCellUpdateError::Sqlx)?;
    Ok(query::value_to_cell(
        &row,
        0,
        row.columns()
            .first()
            .map(|column| column.type_info().name())
            .unwrap_or(target_column.type_name.as_str()),
    ))
}

#[derive(Debug)]
pub enum TableCellUpdateError {
    NotATable,
    MissingPrimaryKey,
    PrimaryKeyEditingUnsupported,
    UnsupportedPrimaryKeyType,
    UnsupportedColumnType,
    NotNullable,
    InvalidCell,
    Sqlx(sqlx::Error),
}

#[derive(Debug)]
pub enum TableFilterError {
    UnknownColumn(String),
    UnsupportedOperator {
        column_name: String,
        operator: FilterOperator,
    },
    MissingValue(String),
}

impl std::fmt::Display for TableFilterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownColumn(column) => write!(formatter, "Unknown filter column: {column}"),
            Self::UnsupportedOperator {
                column_name,
                operator,
            } => {
                write!(
                    formatter,
                    "Operator {} is not supported for column {column_name}",
                    operator.label()
                )
            }
            Self::MissingValue(column) => write!(formatter, "Missing filter value for {column}"),
        }
    }
}

struct TablePageSql {
    sql: String,
    filter_values: Vec<String>,
}

struct TableFilterClause {
    sql: String,
    values: Vec<String>,
}

impl std::fmt::Display for TableCellUpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATable => write!(formatter, "Only tables can be edited."),
            Self::MissingPrimaryKey => write!(formatter, "Editing requires a primary key."),
            Self::PrimaryKeyEditingUnsupported => {
                write!(formatter, "Primary key columns are read-only for now.")
            }
            Self::UnsupportedPrimaryKeyType => {
                write!(
                    formatter,
                    "Editing is not supported for this primary key type yet."
                )
            }
            Self::UnsupportedColumnType => {
                write!(formatter, "This column type is not editable yet.")
            }
            Self::NotNullable => write!(formatter, "This column cannot be set to NULL."),
            Self::InvalidCell => write!(formatter, "The selected cell is no longer available."),
            Self::Sqlx(error) => write!(formatter, "{error}"),
        }
    }
}

#[cfg(test)]
fn table_page_sql(
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
) -> Result<String, TableFilterError> {
    table_page_sql_with_filters(object, columns, offset, page_size, &[]).map(|query| query.sql)
}

fn where_clause(
    columns: &[TableColumn],
    filters: &[TableFilter],
) -> Result<TableFilterClause, TableFilterError> {
    if filters.is_empty() {
        return Ok(TableFilterClause {
            sql: String::new(),
            values: Vec::new(),
        });
    }

    let mut bind_index = 1;
    let mut clauses = Vec::with_capacity(filters.len());
    let mut values = Vec::new();

    for filter in filters {
        let column = columns
            .iter()
            .find(|column| column.name == filter.column_name)
            .ok_or_else(|| TableFilterError::UnknownColumn(filter.column_name.clone()))?;

        if !filter.operator.is_supported_for(column) {
            return Err(TableFilterError::UnsupportedOperator {
                column_name: column.name.clone(),
                operator: filter.operator,
            });
        }

        clauses.push(filter_clause(column, filter, &mut bind_index)?);
        if filter.operator.needs_value() {
            let value = filter
                .value
                .clone()
                .ok_or_else(|| TableFilterError::MissingValue(column.name.clone()))?;

            values.push(value);
        }
    }

    Ok(TableFilterClause {
        sql: format!(" WHERE {}", clauses.join(" AND ")),
        values,
    })
}

fn filter_clause(
    column: &TableColumn,
    filter: &TableFilter,
    bind_index: &mut usize,
) -> Result<String, TableFilterError> {
    let quoted_column = quote_identifier(&column.name);

    match filter.operator {
        FilterOperator::IsNull => Ok(format!("{quoted_column} IS NULL")),
        FilterOperator::IsNotNull => Ok(format!("{quoted_column} IS NOT NULL")),
        operator => {
            let value = filter
                .value
                .as_ref()
                .ok_or_else(|| TableFilterError::MissingValue(column.name.clone()))?;

            if value.is_empty() {
                return Err(TableFilterError::MissingValue(column.name.clone()));
            }

            let (column_expression, value_type) = filter_value_expression(column, operator);
            let clause = format!(
                "{column_expression} {} ${}::{value_type}",
                operator.label(),
                *bind_index,
            );
            *bind_index += 1;
            Ok(clause)
        }
    }
}

fn filter_value_expression(column: &TableColumn, operator: FilterOperator) -> (String, String) {
    let column_name = quote_identifier(&column.name);

    if matches!(operator, FilterOperator::Like | FilterOperator::ILike) {
        return (format!("{column_name}::text"), "text".to_string());
    }

    (column_name, column.display_type.clone())
}

fn table_page_sql_with_filters(
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
    filters: &[TableFilter],
) -> Result<TablePageSql, TableFilterError> {
    let fetch_limit = page_size.saturating_add(1);
    let select_columns = select_columns_clause(columns);
    let where_clause = where_clause(columns, filters)?;
    let order_by = order_by_clause(object, columns);

    Ok(TablePageSql {
        sql: format!(
            "SELECT {select_columns} FROM {}{}{order_by} LIMIT {fetch_limit} OFFSET {offset}",
            object.qualified_name(),
            where_clause.sql,
        ),
        filter_values: where_clause.values,
    })
}

fn update_cell_sql(
    object: &DatabaseObject,
    columns: &[TableColumn],
    column_index: usize,
) -> Result<String, TableCellUpdateError> {
    let target_column = columns
        .get(column_index)
        .ok_or(TableCellUpdateError::InvalidCell)?;
    let primary_key_columns = primary_key_columns(columns);
    if primary_key_columns.is_empty() {
        return Err(TableCellUpdateError::MissingPrimaryKey);
    }

    let assignments = format!(
        "{} = $1::{}",
        quote_identifier(&target_column.name),
        target_column.display_type
    );
    let where_clause = primary_key_columns
        .iter()
        .enumerate()
        .map(|(index, (_, column))| {
            format!(
                "{} IS NOT DISTINCT FROM ${}::{}",
                quote_identifier(&column.name),
                index + 2,
                column.display_type
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");

    Ok(format!(
        "UPDATE {} SET {assignments} WHERE {where_clause} RETURNING {}",
        object.qualified_name(),
        returning_column_expression(target_column)
    ))
}

fn validate_update_input(
    object: &DatabaseObject,
    columns: &[TableColumn],
    row: &[TableCell],
    column_index: usize,
) -> Result<(), TableCellUpdateError> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(TableCellUpdateError::NotATable);
    }

    let Some(column) = columns.get(column_index) else {
        return Err(TableCellUpdateError::InvalidCell);
    };

    if row.len() != columns.len() {
        return Err(TableCellUpdateError::InvalidCell);
    }

    if column.is_primary_key {
        return Err(TableCellUpdateError::PrimaryKeyEditingUnsupported);
    }

    if !column.is_editable_value_type() {
        return Err(TableCellUpdateError::UnsupportedColumnType);
    }

    let primary_key_columns = primary_key_columns(columns);
    if primary_key_columns.is_empty() {
        return Err(TableCellUpdateError::MissingPrimaryKey);
    }

    if primary_key_columns
        .iter()
        .any(|(_, column)| !column.is_editable_value_type())
    {
        return Err(TableCellUpdateError::UnsupportedPrimaryKeyType);
    }

    Ok(())
}

fn primary_key_columns(columns: &[TableColumn]) -> Vec<(usize, &TableColumn)> {
    columns
        .iter()
        .enumerate()
        .filter(|(_, column)| column.is_primary_key)
        .collect()
}

fn select_columns_clause(columns: &[TableColumn]) -> String {
    if columns.is_empty() {
        return "*".to_string();
    }

    columns
        .iter()
        .map(select_column_expression)
        .collect::<Vec<_>>()
        .join(", ")
}

fn select_column_expression(column: &TableColumn) -> String {
    let name = quote_identifier(&column.name);

    if column.uses_text_display() {
        return format!("{name}::text AS {name}");
    }

    name
}

fn returning_column_expression(column: &TableColumn) -> String {
    let name = quote_identifier(&column.name);

    if column.uses_text_display() {
        return format!("{name}::text");
    }

    name
}

fn order_by_clause(object: &DatabaseObject, columns: &[TableColumn]) -> String {
    let primary_key_columns = columns
        .iter()
        .filter(|column| column.is_primary_key)
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>();

    if !primary_key_columns.is_empty() {
        return format!(" ORDER BY {}", primary_key_columns.join(", "));
    }

    if object.kind == DatabaseObjectKind::Table {
        " ORDER BY ctid".to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TableCellUpdateError, TableFilterError, order_by_clause, table_page_sql,
        table_page_sql_with_filters, update_cell_sql, validate_update_input,
    };
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::table_browser::{
        ColumnTypeGroup, FilterOperator, TableCell, TableColumn, TableFilter,
    };

    #[test]
    fn builds_quoted_page_query() {
        let object = DatabaseObject {
            schema: "analytics".to_string(),
            name: "page views".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(
            table_page_sql(&object, &[], 100, 50).unwrap(),
            "SELECT * FROM \"analytics\".\"page views\" ORDER BY ctid LIMIT 51 OFFSET 100"
        );
    }

    #[test]
    fn page_query_uses_limit_plus_one_for_next_page_detection() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(
            table_page_sql(&object, &[], 0, 100).unwrap(),
            "SELECT * FROM \"public\".\"users\" ORDER BY ctid LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn page_query_orders_by_primary_key_when_available() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![
            column("tenant id", true),
            column("id", true),
            column("name", false),
        ];

        assert_eq!(
            table_page_sql(&object, &columns, 0, 100).unwrap(),
            "SELECT \"tenant id\", \"id\", \"name\" FROM \"public\".\"users\" ORDER BY \"tenant id\", \"id\" LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn page_query_orders_by_ctid_for_tables_without_primary_key() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "events".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(
            table_page_sql(&object, &[column("name", false)], 0, 100).unwrap(),
            "SELECT \"name\" FROM \"public\".\"events\" ORDER BY ctid LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn page_query_casts_uuid_and_enum_columns_to_text_for_display() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "orders".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![uuid_column("public_id", true), enum_column("status", false)];

        assert_eq!(
            table_page_sql(&object, &columns, 0, 100).unwrap(),
            "SELECT \"public_id\"::text AS \"public_id\", \"status\"::text AS \"status\" FROM \"public\".\"orders\" ORDER BY \"public_id\" LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn page_query_adds_and_filters_before_ordering() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let mut signup_date = column("signup_date", false);
        signup_date.type_group = ColumnTypeGroup::DateTime;
        signup_date.display_type = "date".to_string();
        signup_date.type_name = "date".to_string();
        let columns = vec![column("id", true), column("name", false), signup_date];

        let query = table_page_sql_with_filters(
            &object,
            &columns,
            0,
            100,
            &[
                filter("name", FilterOperator::ILike, Some("%ada%")),
                filter(
                    "signup_date",
                    FilterOperator::GreaterThanOrEqual,
                    Some("2025-01-01"),
                ),
            ],
        )
        .unwrap();

        assert_eq!(
            query.sql,
            "SELECT \"id\", \"name\", \"signup_date\" FROM \"public\".\"customers\" WHERE \"name\"::text ILIKE $1::text AND \"signup_date\" >= $2::date ORDER BY \"id\" LIMIT 101 OFFSET 0"
        );
        assert_eq!(
            query.filter_values,
            vec!["%ada%".to_string(), "2025-01-01".to_string()]
        );
    }

    #[test]
    fn page_query_uses_null_filter_without_bind_value() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("id", true), column("preferred_contact_time", false)];

        let query = table_page_sql_with_filters(
            &object,
            &columns,
            50,
            50,
            &[filter(
                "preferred_contact_time",
                FilterOperator::IsNull,
                None,
            )],
        )
        .unwrap();

        assert_eq!(
            query.sql,
            "SELECT \"id\", \"preferred_contact_time\" FROM \"public\".\"customers\" WHERE \"preferred_contact_time\" IS NULL ORDER BY \"id\" LIMIT 51 OFFSET 50"
        );
        assert!(query.filter_values.is_empty());
    }

    #[test]
    fn page_query_casts_uuid_and_enum_like_filters_to_text() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "orders".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![
            column("id", true),
            uuid_column("public_id", false),
            enum_column("status", false),
        ];

        let query = table_page_sql_with_filters(
            &object,
            &columns,
            0,
            100,
            &[
                filter("public_id", FilterOperator::Like, Some("8e4%")),
                filter("status", FilterOperator::ILike, Some("%paid%")),
            ],
        )
        .unwrap();

        assert_eq!(
            query.sql,
            "SELECT \"id\", \"public_id\"::text AS \"public_id\", \"status\"::text AS \"status\" FROM \"public\".\"orders\" WHERE \"public_id\"::text LIKE $1::text AND \"status\"::text ILIKE $2::text ORDER BY \"id\" LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn page_query_rejects_invalid_filter_column() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert!(matches!(
            table_page_sql_with_filters(
                &object,
                &[column("id", true)],
                0,
                100,
                &[filter("missing", FilterOperator::Equal, Some("1"))]
            ),
            Err(TableFilterError::UnknownColumn(column)) if column == "missing"
        ));
    }

    #[test]
    fn page_query_rejects_unsupported_filter_operator() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "events".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let mut payload = column("payload", false);
        payload.type_group = ColumnTypeGroup::Json;
        payload.display_type = "jsonb".to_string();
        payload.type_name = "jsonb".to_string();

        assert!(matches!(
            table_page_sql_with_filters(
                &object,
                &[column("id", true), payload],
                0,
                100,
                &[filter("payload", FilterOperator::Equal, Some("{}"))]
            ),
            Err(TableFilterError::UnsupportedOperator { .. })
        ));
    }

    #[test]
    fn order_by_is_empty_for_views_without_primary_key() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "event_summary".to_string(),
            kind: DatabaseObjectKind::View,
        };

        assert_eq!(order_by_clause(&object, &[column("name", false)]), "");
    }

    #[test]
    fn update_cell_query_uses_primary_key_and_returns_updated_value() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("id", true), column("display name", false)];

        assert_eq!(
            update_cell_sql(&object, &columns, 1).unwrap(),
            "UPDATE \"public\".\"users\" SET \"display name\" = $1::text WHERE \"id\" IS NOT DISTINCT FROM $2::text RETURNING \"display name\""
        );
    }

    #[test]
    fn update_cell_query_supports_composite_primary_keys() {
        let object = DatabaseObject {
            schema: "tenant data".to_string(),
            name: "settings".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![
            column("tenant id", true),
            column("key", true),
            column("value", false),
        ];

        assert_eq!(
            update_cell_sql(&object, &columns, 2).unwrap(),
            "UPDATE \"tenant data\".\"settings\" SET \"value\" = $1::text WHERE \"tenant id\" IS NOT DISTINCT FROM $2::text AND \"key\" IS NOT DISTINCT FROM $3::text RETURNING \"value\""
        );
    }

    #[test]
    fn update_cell_query_returns_uuid_and_enum_values_as_text() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "orders".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(
            update_cell_sql(
                &object,
                &[column("id", true), uuid_column("public_id", false)],
                1
            )
            .unwrap(),
            "UPDATE \"public\".\"orders\" SET \"public_id\" = $1::uuid WHERE \"id\" IS NOT DISTINCT FROM $2::text RETURNING \"public_id\"::text"
        );

        assert_eq!(
            update_cell_sql(
                &object,
                &[column("id", true), enum_column("status", false)],
                1
            )
            .unwrap(),
            "UPDATE \"public\".\"orders\" SET \"status\" = $1::public.order_status WHERE \"id\" IS NOT DISTINCT FROM $2::text RETURNING \"status\"::text"
        );
    }

    #[test]
    fn update_cell_query_rejects_unknown_column() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert!(matches!(
            update_cell_sql(&object, &[column("id", true)], 3),
            Err(TableCellUpdateError::InvalidCell)
        ));
    }

    #[test]
    fn update_cell_query_rejects_missing_primary_key() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert!(matches!(
            update_cell_sql(&object, &[column("name", false)], 0),
            Err(TableCellUpdateError::MissingPrimaryKey)
        ));
    }

    #[test]
    fn cell_update_rejects_unsupported_primary_key_types() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "files".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let mut id = column("id", true);
        id.type_group = ColumnTypeGroup::Binary;
        id.display_type = "bytea".to_string();
        id.type_name = "bytea".to_string();
        let columns = vec![id, column("name", false)];
        let row = vec![
            TableCell::new("\\xdeadbeef".to_string()),
            TableCell::new("readme".to_string()),
        ];

        assert!(matches!(
            validate_update_input(&object, &columns, &row, 1),
            Err(TableCellUpdateError::UnsupportedPrimaryKeyType)
        ));
    }

    fn column(name: &str, is_primary_key: bool) -> TableColumn {
        TableColumn {
            name: name.to_string(),
            display_type: "text".to_string(),
            type_name: "text".to_string(),
            enum_values: Vec::new(),
            type_group: ColumnTypeGroup::Text,
            is_nullable: false,
            is_primary_key,
            ordinal_position: 1,
        }
    }

    fn uuid_column(name: &str, is_primary_key: bool) -> TableColumn {
        TableColumn {
            display_type: "uuid".to_string(),
            type_name: "uuid".to_string(),
            type_group: ColumnTypeGroup::Other,
            ..column(name, is_primary_key)
        }
    }

    fn enum_column(name: &str, is_primary_key: bool) -> TableColumn {
        TableColumn {
            display_type: "public.order_status".to_string(),
            type_name: "order_status".to_string(),
            enum_values: vec!["draft".to_string(), "paid".to_string()],
            type_group: ColumnTypeGroup::Other,
            ..column(name, is_primary_key)
        }
    }

    fn filter(column_name: &str, operator: FilterOperator, value: Option<&str>) -> TableFilter {
        TableFilter {
            column_name: column_name.to_string(),
            operator,
            value: value.map(str::to_string),
        }
    }
}
