use crate::db::query;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind, quote_identifier};
use crate::models::table_browser::{TableCell, TableColumn};
use sqlx::{Column, PgPool, Row, TypeInfo};

use super::page_sql::{primary_key_columns, returning_column_expression};

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

pub(super) fn update_cell_sql(
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

pub(super) fn validate_update_input(
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
