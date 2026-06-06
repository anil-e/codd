use sqlx::PgPool;

use crate::models::database_object::DatabaseObject;
use crate::models::table_structure::TableIndex;

#[derive(Debug, sqlx::FromRow)]
struct TableIndexRow {
    schema: String,
    name: String,
    method: String,
    definition: String,
    predicate: Option<String>,
    is_unique: bool,
    is_primary: bool,
    is_valid: bool,
    is_constraint_backed: bool,
}

pub(super) async fn load_indexes(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableIndex>, sqlx::Error> {
    sqlx::query_as::<_, TableIndexRow>(
        r"
        SELECT
            idx_ns.nspname AS schema,
            idx.relname AS name,
            am.amname AS method,
            pg_get_indexdef(idx.oid, 0, true) AS definition,
            pg_get_expr(i.indpred, i.indrelid) AS predicate,
            i.indisunique AS is_unique,
            i.indisprimary AS is_primary,
            i.indisvalid AS is_valid,
            EXISTS (
                SELECT 1
                FROM pg_constraint con
                WHERE con.conindid = idx.oid
            ) AS is_constraint_backed
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_index i ON i.indrelid = c.oid
        JOIN pg_class idx ON idx.oid = i.indexrelid
        JOIN pg_namespace idx_ns ON idx_ns.oid = idx.relnamespace
        JOIN pg_am am ON am.oid = idx.relam
        WHERE n.nspname = $1
          AND c.relname = $2
          AND c.relkind IN ('r', 'p')
        ORDER BY i.indisprimary DESC, idx.relname
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| TableIndex {
                schema: row.schema,
                name: row.name,
                method: row.method,
                definition: row.definition,
                predicate: row.predicate,
                is_unique: row.is_unique,
                is_primary: row.is_primary,
                is_valid: row.is_valid,
                is_constraint_backed: row.is_constraint_backed,
            })
            .collect()
    })
}
