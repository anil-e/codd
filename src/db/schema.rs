use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use sqlx::{PgPool, Row};

#[derive(Debug)]
struct SchemaObjectRow {
    schema: String,
    name: String,
    table_type: String,
}

pub async fn load_schema(pool: &PgPool) -> Result<Vec<DatabaseObject>, sqlx::Error> {
    let rows = sqlx::query(
        r"
        SELECT
            table_schema AS schema,
            table_name AS name,
            table_type
        FROM information_schema.tables
        WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
          AND table_type IN ('BASE TABLE', 'VIEW')
        ORDER BY table_schema, table_type, table_name
        ",
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let row = SchemaObjectRow {
                schema: row.try_get("schema")?,
                name: row.try_get("name")?,
                table_type: row.try_get("table_type")?,
            };

            Ok(DatabaseObject {
                schema: row.schema,
                name: row.name,
                kind: match row.table_type.as_str() {
                    "VIEW" => DatabaseObjectKind::View,
                    _ => DatabaseObjectKind::Table,
                },
            })
        })
        .collect()
}
