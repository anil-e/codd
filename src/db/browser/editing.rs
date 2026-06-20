use crate::db::query;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind, quote_identifier};
use crate::models::table_browser::{TableCell, TableColumn, TableInsertValue};
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

pub async fn insert_table_row(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    values: &[TableInsertValue],
) -> Result<(), TableRowInsertError> {
    validate_insert_input(object, columns, values)?;

    let sql = insert_row_sql(object, columns, values)?;
    let mut query = sqlx::query(&sql);

    for value in values {
        match value {
            TableInsertValue::Default => {}
            TableInsertValue::Null => {
                query = query.bind(Option::<String>::None);
            }
            TableInsertValue::Value(value) => {
                query = query.bind(Some(value.clone()));
            }
        }
    }

    query
        .execute(pool)
        .await
        .map_err(TableRowInsertError::Sqlx)?;

    Ok(())
}

pub async fn delete_table_row(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    row: &[TableCell],
) -> Result<(), TableRowDeleteError> {
    validate_delete_input(object, columns, row)?;

    let sql = delete_row_sql(object, columns)?;
    let mut query = sqlx::query(&sql);

    for (index, _) in primary_key_columns(columns) {
        let cell = &row[index];
        let value = (!cell.is_null).then(|| cell.value.clone());
        query = query.bind(value);
    }

    let result = query
        .execute(pool)
        .await
        .map_err(TableRowDeleteError::Sqlx)?;

    if result.rows_affected() != 1 {
        return Err(TableRowDeleteError::InvalidRow);
    }

    Ok(())
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
pub enum TableRowInsertError {
    NotATable,
    InvalidColumnValues,
    MissingRequiredValue(String),
    UnsupportedColumnType(String),
    Sqlx(sqlx::Error),
}

#[derive(Debug)]
pub enum TableRowDeleteError {
    NotATable,
    MissingPrimaryKey,
    UnsupportedPrimaryKeyType,
    InvalidRow,
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

impl std::fmt::Display for TableRowInsertError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATable => write!(formatter, "Only tables can be edited."),
            Self::InvalidColumnValues => write!(formatter, "The submitted row is no longer valid."),
            Self::MissingRequiredValue(column) => {
                write!(formatter, "Missing value for required column {column}.")
            }
            Self::UnsupportedColumnType(column) => {
                write!(
                    formatter,
                    "Column {column} cannot be inserted from this form."
                )
            }
            Self::Sqlx(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::fmt::Display for TableRowDeleteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATable => write!(formatter, "Only tables can be edited."),
            Self::MissingPrimaryKey => write!(formatter, "Deleting requires a primary key."),
            Self::UnsupportedPrimaryKeyType => {
                write!(
                    formatter,
                    "Deleting is not supported for this primary key type yet."
                )
            }
            Self::InvalidRow => write!(formatter, "The selected row is no longer available."),
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

pub(super) fn insert_row_sql(
    object: &DatabaseObject,
    columns: &[TableColumn],
    values: &[TableInsertValue],
) -> Result<String, TableRowInsertError> {
    validate_insert_input(object, columns, values)?;

    let mut insert_columns = Vec::new();
    let mut insert_values = Vec::new();
    let mut bind_index = 1;

    for (column, value) in columns.iter().zip(values) {
        if matches!(value, TableInsertValue::Default) {
            continue;
        }

        insert_columns.push(quote_identifier(&column.name));
        insert_values.push(format!("${bind_index}::{}", column.display_type));
        bind_index += 1;
    }

    if insert_columns.is_empty() {
        return Ok(format!(
            "INSERT INTO {} DEFAULT VALUES",
            object.qualified_name()
        ));
    }

    Ok(format!(
        "INSERT INTO {} ({}) VALUES ({})",
        object.qualified_name(),
        insert_columns.join(", "),
        insert_values.join(", ")
    ))
}

pub(super) fn delete_row_sql(
    object: &DatabaseObject,
    columns: &[TableColumn],
) -> Result<String, TableRowDeleteError> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(TableRowDeleteError::NotATable);
    }

    let primary_key_columns = primary_key_columns(columns);
    if primary_key_columns.is_empty() {
        return Err(TableRowDeleteError::MissingPrimaryKey);
    }

    if primary_key_columns
        .iter()
        .any(|(_, column)| !column.is_editable_value_type())
    {
        return Err(TableRowDeleteError::UnsupportedPrimaryKeyType);
    }

    let where_clause = primary_key_columns
        .iter()
        .enumerate()
        .map(|(bind_index, (_, column))| {
            format!(
                "{} IS NOT DISTINCT FROM ${}::{}",
                quote_identifier(&column.name),
                bind_index + 1,
                column.display_type
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ");

    Ok(format!(
        "DELETE FROM {} WHERE {where_clause}",
        object.qualified_name()
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

pub(super) fn validate_insert_input(
    object: &DatabaseObject,
    columns: &[TableColumn],
    values: &[TableInsertValue],
) -> Result<(), TableRowInsertError> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(TableRowInsertError::NotATable);
    }

    if columns.len() != values.len() {
        return Err(TableRowInsertError::InvalidColumnValues);
    }

    for (column, value) in columns.iter().zip(values) {
        match value {
            TableInsertValue::Default => {
                if column.is_required_for_insert() {
                    return Err(TableRowInsertError::MissingRequiredValue(
                        column.name.clone(),
                    ));
                }
            }
            TableInsertValue::Null => {
                if !column.is_insertable() {
                    return Err(TableRowInsertError::UnsupportedColumnType(
                        column.name.clone(),
                    ));
                }

                if !column.is_nullable {
                    return Err(TableRowInsertError::MissingRequiredValue(
                        column.name.clone(),
                    ));
                }
            }
            TableInsertValue::Value(_) => {
                if !column.is_insertable() {
                    return Err(TableRowInsertError::UnsupportedColumnType(
                        column.name.clone(),
                    ));
                }
            }
        }
    }

    Ok(())
}

pub(super) fn validate_delete_input(
    object: &DatabaseObject,
    columns: &[TableColumn],
    row: &[TableCell],
) -> Result<(), TableRowDeleteError> {
    if row.len() != columns.len() {
        return Err(TableRowDeleteError::InvalidRow);
    }

    delete_row_sql(object, columns).map(|_| ())
}
