use crate::db::query;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind, quote_identifier};
use crate::models::table_browser::{ColumnTypeGroup, TableColumn, TablePage};
use sqlx::{Column, PgPool, Row, TypeInfo};

pub async fn load_table_page(
    pool: &PgPool,
    object: &DatabaseObject,
    offset: u32,
    page_size: u32,
) -> Result<TablePage, sqlx::Error> {
    let columns = load_table_columns(pool, object).await?;
    let mut rows = load_page_rows(pool, object, &columns, offset, page_size).await?;
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
    let rows = sqlx::query_as::<_, (String, String, String, bool, bool, i32)>(
        r"
        SELECT
            a.attname,
            format_type(a.atttypid, a.atttypmod) AS display_type,
            t.typname,
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
        WHERE n.nspname = $1
          AND c.relname = $2
          AND a.attnum > 0
          AND NOT a.attisdropped
          AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
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
            |(name, display_type, type_name, is_nullable, is_primary_key, ordinal_position)| {
                TableColumn {
                    name,
                    display_type,
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
) -> Result<Vec<Vec<String>>, sqlx::Error> {
    let sql = table_page_sql(object, columns, offset, page_size);
    let rows = sqlx::query(&sql).fetch_all(pool).await?;

    Ok(rows
        .iter()
        .map(|row| {
            row.columns()
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    query::value_to_string(row, index, column.type_info().name())
                })
                .collect()
        })
        .collect())
}

fn table_page_sql(
    object: &DatabaseObject,
    columns: &[TableColumn],
    offset: u32,
    page_size: u32,
) -> String {
    let fetch_limit = page_size.saturating_add(1);
    let order_by = order_by_clause(object, columns);

    format!(
        "SELECT * FROM {}{order_by} LIMIT {fetch_limit} OFFSET {offset}",
        object.qualified_name()
    )
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
    use super::{order_by_clause, table_page_sql};
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::table_browser::{ColumnTypeGroup, TableColumn};

    #[test]
    fn builds_quoted_page_query() {
        let object = DatabaseObject {
            schema: "analytics".to_string(),
            name: "page views".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        assert_eq!(
            table_page_sql(&object, &[], 100, 50),
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
            table_page_sql(&object, &[], 0, 100),
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
            table_page_sql(&object, &columns, 0, 100),
            "SELECT * FROM \"public\".\"users\" ORDER BY \"tenant id\", \"id\" LIMIT 101 OFFSET 0"
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
            table_page_sql(&object, &[column("name", false)], 0, 100),
            "SELECT * FROM \"public\".\"events\" ORDER BY ctid LIMIT 101 OFFSET 0"
        );
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

    fn column(name: &str, is_primary_key: bool) -> TableColumn {
        TableColumn {
            name: name.to_string(),
            display_type: "text".to_string(),
            type_name: "text".to_string(),
            type_group: ColumnTypeGroup::Text,
            is_nullable: false,
            is_primary_key,
            ordinal_position: 1,
        }
    }
}
