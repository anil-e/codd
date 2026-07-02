use std::collections::BTreeMap;

use crate::models::completion::{
    CompletionCatalog, CompletionColumn, CompletionItemKind, CompletionObject, CompletionSchema,
};
use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
struct CompletionObjectRow {
    schema: String,
    name: String,
    relkind: String,
}

#[derive(Debug, sqlx::FromRow)]
struct CompletionColumnRow {
    schema: String,
    object_name: String,
    name: String,
    data_type: String,
    ordinal_position: i32,
}

pub async fn load_catalog(pool: &PgPool) -> Result<CompletionCatalog, sqlx::Error> {
    let object_rows = load_objects(pool).await?;
    let column_rows = load_columns(pool).await?;

    let schemas = completion_schemas(&object_rows);
    let objects = completion_objects(object_rows, column_rows);

    Ok(CompletionCatalog { schemas, objects })
}

async fn load_objects(pool: &PgPool) -> Result<Vec<CompletionObjectRow>, sqlx::Error> {
    sqlx::query_as::<_, CompletionObjectRow>(
        r"
        SELECT
            n.nspname AS schema,
            c.relname AS name,
            c.relkind::text AS relkind
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
          AND c.relkind IN ('r', 'p', 'v', 'm')
        ORDER BY n.nspname, c.relkind, c.relname
        ",
    )
    .fetch_all(pool)
    .await
}

async fn load_columns(pool: &PgPool) -> Result<Vec<CompletionColumnRow>, sqlx::Error> {
    sqlx::query_as::<_, CompletionColumnRow>(
        r"
        SELECT
            n.nspname AS schema,
            c.relname AS object_name,
            a.attname AS name,
            format_type(a.atttypid, a.atttypmod) AS data_type,
            a.attnum::int4 AS ordinal_position
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_attribute a ON a.attrelid = c.oid
        WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
          AND c.relkind IN ('r', 'p', 'v', 'm')
          AND a.attnum > 0
          AND NOT a.attisdropped
        ORDER BY n.nspname, c.relname, a.attnum
        ",
    )
    .fetch_all(pool)
    .await
}

fn completion_schemas(rows: &[CompletionObjectRow]) -> Vec<CompletionSchema> {
    let mut schemas = rows
        .iter()
        .map(|row| row.schema.clone())
        .collect::<Vec<_>>();
    schemas.sort();
    schemas.dedup();

    schemas
        .into_iter()
        .map(|name| CompletionSchema { name })
        .collect()
}

fn completion_objects(
    object_rows: Vec<CompletionObjectRow>,
    column_rows: Vec<CompletionColumnRow>,
) -> Vec<CompletionObject> {
    let mut columns_by_object = BTreeMap::new();
    for column in column_rows {
        columns_by_object
            .entry((column.schema, column.object_name))
            .or_insert_with(Vec::new)
            .push(CompletionColumn {
                name: column.name,
                data_type: column.data_type,
                ordinal_position: column.ordinal_position,
            });
    }

    object_rows
        .into_iter()
        .map(|row| {
            let columns = columns_by_object
                .remove(&(row.schema.clone(), row.name.clone()))
                .unwrap_or_default();

            CompletionObject {
                schema: row.schema,
                name: row.name,
                kind: completion_object_kind(&row.relkind),
                columns,
            }
        })
        .collect()
}

fn completion_object_kind(relkind: &str) -> CompletionItemKind {
    match relkind {
        "v" => CompletionItemKind::View,
        "m" => CompletionItemKind::MaterializedView,
        _ => CompletionItemKind::Table,
    }
}

#[cfg(test)]
mod tests {
    use super::{CompletionColumnRow, CompletionObjectRow, completion_objects, completion_schemas};
    use crate::models::completion::CompletionItemKind;

    #[test]
    fn builds_deduplicated_schemas_from_objects() {
        let schemas = completion_schemas(&[
            object_row("public", "users", "r"),
            object_row("public", "orders", "r"),
            object_row("analytics", "page_views", "m"),
        ]);

        assert_eq!(schemas[0].name, "analytics");
        assert_eq!(schemas[1].name, "public");
    }

    #[test]
    fn maps_objects_and_columns() {
        let objects = completion_objects(
            vec![
                object_row("public", "users", "r"),
                object_row("analytics", "page_views", "m"),
            ],
            vec![
                column_row("public", "users", "id", "bigint", 1),
                column_row("public", "users", "email", "text", 2),
                column_row("analytics", "page_views", "path", "text", 1),
            ],
        );

        assert_eq!(objects[0].kind, CompletionItemKind::Table);
        assert_eq!(objects[0].columns.len(), 2);
        assert_eq!(objects[1].kind, CompletionItemKind::MaterializedView);
        assert_eq!(objects[1].columns[0].name, "path");
    }

    fn object_row(schema: &str, name: &str, relkind: &str) -> CompletionObjectRow {
        CompletionObjectRow {
            schema: schema.to_string(),
            name: name.to_string(),
            relkind: relkind.to_string(),
        }
    }

    fn column_row(
        schema: &str,
        object_name: &str,
        name: &str,
        data_type: &str,
        ordinal_position: i32,
    ) -> CompletionColumnRow {
        CompletionColumnRow {
            schema: schema.to_string(),
            object_name: object_name.to_string(),
            name: name.to_string(),
            data_type: data_type.to_string(),
            ordinal_position,
        }
    }
}
