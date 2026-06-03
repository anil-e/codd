use crate::db::query;
use crate::models::csv_export::CsvExportOptions;
use crate::models::database_object::DatabaseObject;
use crate::models::table_browser::{TableCell, TableColumn, TableFilter, TablePage, TableSort};
use sqlx::{Column, PgPool, Row, TypeInfo};

mod columns;
mod editing;
mod page_sql;

pub use columns::load_table_columns;
pub use editing::update_table_cell;

pub async fn load_table_page(
    pool: &PgPool,
    object: &DatabaseObject,
    offset: u32,
    page_size: u32,
    filters: &[TableFilter],
    sort: Option<&TableSort>,
) -> Result<TablePage, sqlx::Error> {
    let columns = load_table_columns(pool, object).await?;
    let mut rows = load_page_rows(pool, object, &columns, offset, page_size, filters, sort).await?;
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

pub async fn load_table_row_count(
    pool: &PgPool,
    object: &DatabaseObject,
    filters: &[TableFilter],
) -> Result<i64, sqlx::Error> {
    let columns = load_table_columns(pool, object).await?;
    let query = page_sql::table_count_sql_with_filters(object, &columns, filters)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let mut sqlx_query = sqlx::query_scalar::<_, i64>(&query.sql);

    for value in query.filter_values {
        sqlx_query = sqlx_query.bind(value);
    }

    sqlx_query.fetch_one(pool).await
}

pub async fn export_table_page(
    pool: &PgPool,
    object: &DatabaseObject,
    filters: &[TableFilter],
    sort: Option<&TableSort>,
    options: CsvExportOptions,
) -> Result<TablePage, sqlx::Error> {
    let page_size = u32::try_from(options.row_limit).unwrap_or(u32::MAX);
    let columns = load_table_columns(pool, object).await?;
    let rows = load_export_rows(pool, object, &columns, page_size, filters, sort).await?;

    Ok(TablePage {
        object: object.clone(),
        columns,
        rows,
        offset: 0,
        page_size,
        has_next_page: false,
    })
}

async fn load_page_rows(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
    filters: &[TableFilter],
    sort: Option<&TableSort>,
) -> Result<Vec<Vec<TableCell>>, sqlx::Error> {
    let query =
        page_sql::table_page_sql_with_filters(object, columns, offset, page_size, filters, sort)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let mut sqlx_query = sqlx::query(&query.sql);

    for value in query.filter_values {
        sqlx_query = sqlx_query.bind(value);
    }

    let rows = sqlx_query.fetch_all(pool).await?;

    let rows = rows
        .iter()
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(index, column)| query::value_to_cell(row, index, column.type_info().name()))
                .collect()
        })
        .collect();

    Ok(rows)
}

async fn load_export_rows(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    page_size: u32,
    filters: &[TableFilter],
    sort: Option<&TableSort>,
) -> Result<Vec<Vec<TableCell>>, sqlx::Error> {
    let query = page_sql::table_export_sql_with_filters(object, columns, page_size, filters, sort)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    let mut sqlx_query = sqlx::query(&query.sql);

    for value in query.filter_values {
        sqlx_query = sqlx_query.bind(value);
    }

    let rows = sqlx_query.fetch_all(pool).await?;

    let rows = rows
        .iter()
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    query::value_to_export_cell(row, index, column.type_info().name())
                })
                .collect()
        })
        .collect();

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::{
        editing::{TableCellUpdateError, update_cell_sql, validate_update_input},
        page_sql::{
            TableFilterError, order_by_clause, table_count_sql_with_filters,
            table_export_sql_with_filters, table_page_sql, table_page_sql_with_filters,
        },
    };
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::table_browser::{
        ColumnTypeGroup, FilterOperator, SortDirection, TableCell, TableColumn, TableFilter,
        TableSort,
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
    fn export_query_uses_exact_limit() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(
            table_export_sql_with_filters(&object, &[], 100, &[], None)
                .unwrap()
                .sql,
            "SELECT * FROM \"public\".\"users\" ORDER BY ctid LIMIT 100 OFFSET 0"
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
    fn page_query_casts_text_display_columns_to_text() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "orders".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![
            uuid_column("public_id", true),
            enum_column("status", false),
            numeric_column("amount", false),
        ];

        assert_eq!(
            table_page_sql(&object, &columns, 0, 100).unwrap(),
            "SELECT \"public_id\"::text AS \"public_id\", \"status\"::text AS \"status\", \"amount\"::text AS \"amount\" FROM \"public\".\"orders\" ORDER BY \"public_id\" LIMIT 101 OFFSET 0"
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
            None,
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
            None,
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
            None,
        )
        .unwrap();

        assert_eq!(
            query.sql,
            "SELECT \"id\", \"public_id\"::text AS \"public_id\", \"status\"::text AS \"status\" FROM \"public\".\"orders\" WHERE \"public_id\"::text LIKE $1::text AND \"status\"::text ILIKE $2::text ORDER BY \"id\" LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn page_query_adds_custom_sql_filters_without_bind_values() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("id", true), column("name", false)];

        let query = table_page_sql_with_filters(
            &object,
            &columns,
            0,
            100,
            &[TableFilter::custom_sql("lower(name) LIKE 'a%'")],
            None,
        )
        .unwrap();

        assert_eq!(
            query.sql,
            "SELECT \"id\", \"name\" FROM \"public\".\"customers\" WHERE (lower(name) LIKE 'a%') ORDER BY \"id\" LIMIT 101 OFFSET 0"
        );
        assert!(query.filter_values.is_empty());
    }

    #[test]
    fn count_query_reuses_filters() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("id", true), column("name", false)];
        let filter = TableFilter::column(
            "name".to_string(),
            FilterOperator::ILike,
            Some("%Ada%".to_string()),
        );

        let query = table_count_sql_with_filters(&object, &columns, &[filter]).unwrap();

        assert_eq!(
            query.sql,
            "SELECT COUNT(*) FROM \"public\".\"customers\" WHERE \"name\"::text ILIKE $1::text"
        );
        assert_eq!(query.filter_values, vec!["%Ada%"]);
    }

    #[test]
    fn page_query_rejects_unsafe_custom_sql_filters() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("id", true), column("name", false)];

        assert!(matches!(
            table_page_sql_with_filters(
                &object,
                &columns,
                0,
                100,
                &[TableFilter::custom_sql("true; DROP TABLE users")],
                None
            ),
            Err(TableFilterError::InvalidCustomSql)
        ));

        assert!(matches!(
            table_page_sql_with_filters(
                &object,
                &columns,
                0,
                100,
                &[TableFilter::custom_sql("name = $1")],
                None
            ),
            Err(TableFilterError::InvalidCustomSql)
        ));
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
                &[filter("missing", FilterOperator::Equal, Some("1"))],
                None
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
                &[filter("payload", FilterOperator::Equal, Some("{}"))],
                None
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

        assert_eq!(order_by_clause(&object, &[column("name", false)], None), "");
    }

    #[test]
    fn page_query_orders_by_selected_column_before_stable_fallback() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "customers".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("id", true), column("name", false)];
        let sort = TableSort::new("name".to_string(), SortDirection::Descending);

        let query =
            table_page_sql_with_filters(&object, &columns, 0, 100, &[], Some(&sort)).unwrap();

        assert_eq!(
            query.sql,
            "SELECT \"id\", \"name\" FROM \"public\".\"customers\" ORDER BY \"name\" DESC, \"id\" LIMIT 101 OFFSET 0"
        );
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
    fn update_cell_query_returns_text_display_values_as_text() {
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

        assert_eq!(
            update_cell_sql(
                &object,
                &[column("id", true), numeric_column("amount", false)],
                1
            )
            .unwrap(),
            "UPDATE \"public\".\"orders\" SET \"amount\" = $1::numeric(14,4) WHERE \"id\" IS NOT DISTINCT FROM $2::text RETURNING \"amount\"::text"
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

    fn numeric_column(name: &str, is_primary_key: bool) -> TableColumn {
        TableColumn {
            display_type: "numeric(14,4)".to_string(),
            type_name: "numeric".to_string(),
            type_group: ColumnTypeGroup::Numeric,
            ..column(name, is_primary_key)
        }
    }

    fn filter(column_name: &str, operator: FilterOperator, value: Option<&str>) -> TableFilter {
        TableFilter::column(column_name.to_owned(), operator, value.map(str::to_string))
    }
}
