use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use sqlx::PgPool;

pub async fn load_schema(pool: &PgPool) -> Result<Vec<DatabaseObject>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        r"
        SELECT table_schema, table_name, table_type
        FROM information_schema.tables
        WHERE table_schema NOT IN ('pg_catalog', 'information_schema')
          AND table_type IN ('BASE TABLE', 'VIEW')
        ORDER BY table_schema, table_type, table_name
        ",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(schema, name, table_type)| DatabaseObject {
            schema,
            name,
            kind: match table_type.as_str() {
                "VIEW" => DatabaseObjectKind::View,
                _ => DatabaseObjectKind::Table,
            },
        })
        .collect())
}
