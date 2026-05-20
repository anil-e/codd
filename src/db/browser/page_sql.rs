use crate::models::database_object::{DatabaseObject, DatabaseObjectKind, quote_identifier};
use crate::models::table_browser::{FilterOperator, TableColumn, TableFilter, TableSort};

pub(super) struct TableBrowserSql {
    pub(super) sql: String,
    pub(super) filter_values: Vec<String>,
}

struct TableFilterClause {
    sql: String,
    values: Vec<String>,
}

#[derive(Debug)]
pub(super) enum TableFilterError {
    UnknownColumn(String),
    UnsupportedOperator {
        column_name: String,
        operator: FilterOperator,
    },
    MissingValue(String),
    InvalidCustomSql,
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
            Self::InvalidCustomSql => write!(formatter, "Invalid custom SQL filter"),
        }
    }
}

#[cfg(test)]
pub(super) fn table_page_sql(
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
) -> Result<String, TableFilterError> {
    table_page_sql_with_filters(object, columns, offset, page_size, &[], None)
        .map(|query| query.sql)
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
        match filter {
            TableFilter::CustomSql { expression } => {
                let expression = expression.trim();
                if expression.is_empty() || !is_valid_custom_sql_filter(expression) {
                    return Err(TableFilterError::InvalidCustomSql);
                }

                clauses.push(format!("({expression})"));
            }

            TableFilter::Column {
                column_name,
                operator,
                value,
            } => {
                let column = columns
                    .iter()
                    .find(|column| column.name == *column_name)
                    .ok_or_else(|| TableFilterError::UnknownColumn(column_name.clone()))?;

                if !operator.is_supported_for(column) {
                    return Err(TableFilterError::UnsupportedOperator {
                        column_name: column.name.clone(),
                        operator: *operator,
                    });
                }

                clauses.push(filter_clause(
                    column,
                    *operator,
                    value.as_deref(),
                    &mut bind_index,
                )?);

                if operator.needs_value() {
                    let value = value
                        .clone()
                        .ok_or_else(|| TableFilterError::MissingValue(column.name.clone()))?;

                    values.push(value);
                }
            }
        }
    }

    Ok(TableFilterClause {
        sql: format!(" WHERE {}", clauses.join(" AND ")),
        values,
    })
}

fn is_valid_custom_sql_filter(expression: &str) -> bool {
    !expression.contains(';')
        && !expression.contains("--")
        && !expression.contains("/*")
        && !contains_positional_parameter(expression)
}

fn contains_positional_parameter(expression: &str) -> bool {
    expression
        .as_bytes()
        .windows(2)
        .any(|window| window[0] == b'$' && window[1].is_ascii_digit())
}

fn filter_clause(
    column: &TableColumn,
    operator: FilterOperator,
    value: Option<&str>,
    bind_index: &mut usize,
) -> Result<String, TableFilterError> {
    let quoted_column = quote_identifier(&column.name);

    match operator {
        FilterOperator::IsNull => Ok(format!("{quoted_column} IS NULL")),
        FilterOperator::IsNotNull => Ok(format!("{quoted_column} IS NOT NULL")),

        operator => {
            let value = value.ok_or_else(|| TableFilterError::MissingValue(column.name.clone()))?;

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

pub(super) fn table_page_sql_with_filters(
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
    filters: &[TableFilter],
    sort: Option<&TableSort>,
) -> Result<TableBrowserSql, TableFilterError> {
    let fetch_limit = page_size.saturating_add(1);
    let select_columns = select_columns_clause(columns);
    let where_clause = where_clause(columns, filters)?;
    let order_by = order_by_clause(object, columns, sort);

    Ok(TableBrowserSql {
        sql: format!(
            "SELECT {select_columns} FROM {}{}{order_by} LIMIT {fetch_limit} OFFSET {offset}",
            object.qualified_name(),
            where_clause.sql,
        ),
        filter_values: where_clause.values,
    })
}

pub(super) fn table_count_sql_with_filters(
    object: &DatabaseObject,
    columns: &[TableColumn],
    filters: &[TableFilter],
) -> Result<TableBrowserSql, TableFilterError> {
    let where_clause = where_clause(columns, filters)?;

    Ok(TableBrowserSql {
        sql: format!(
            "SELECT COUNT(*) FROM {}{}",
            object.qualified_name(),
            where_clause.sql,
        ),
        filter_values: where_clause.values,
    })
}

pub(super) fn primary_key_columns(columns: &[TableColumn]) -> Vec<(usize, &TableColumn)> {
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

pub(super) fn returning_column_expression(column: &TableColumn) -> String {
    let name = quote_identifier(&column.name);

    if column.uses_text_display() {
        return format!("{name}::text");
    }

    name
}

pub(super) fn order_by_clause(
    object: &DatabaseObject,
    columns: &[TableColumn],
    sort: Option<&TableSort>,
) -> String {
    let mut expressions = Vec::new();

    if let Some(sort) = sort
        && columns.iter().any(|column| column.name == sort.column_name)
    {
        expressions.push(format!(
            "{} {}",
            quote_identifier(&sort.column_name),
            sort.direction.sql()
        ));
    }

    let primary_key_columns = columns
        .iter()
        .filter(|column| column.is_primary_key)
        .filter(|column| sort.is_none_or(|sort| sort.column_name != column.name))
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>();

    if !primary_key_columns.is_empty() {
        expressions.extend(primary_key_columns);
    } else if object.kind == DatabaseObjectKind::Table
        && sort.is_none_or(|sort| sort.column_name != "ctid")
    {
        expressions.push("ctid".to_string());
    }

    if expressions.is_empty() {
        return String::new();
    }

    format!(" ORDER BY {}", expressions.join(", "))
}
