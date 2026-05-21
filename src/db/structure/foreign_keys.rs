use sqlx::PgPool;

use crate::models::database_object::DatabaseObject;
use crate::models::table_structure::{ForeignKeyAction, TableForeignKey};

#[derive(Debug, sqlx::FromRow)]
struct TableForeignKeyRow {
    name: String,
    columns: Vec<String>,
    referenced_schema: String,
    referenced_table: String,
    referenced_columns: Vec<String>,
    on_update: String,
    on_delete: String,
    is_deferrable: bool,
    is_initially_deferred: bool,
}

pub(super) async fn load_foreign_keys(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableForeignKey>, sqlx::Error> {
    sqlx::query_as::<_, TableForeignKeyRow>(
        r"
        SELECT
            con.conname AS name,
            local_columns.names AS columns,
            ref_ns.nspname AS referenced_schema,
            ref_class.relname AS referenced_table,
            referenced_columns.names AS referenced_columns,
            con.confupdtype::text AS on_update,
            con.confdeltype::text AS on_delete,
            con.condeferrable AS is_deferrable,
            con.condeferred AS is_initially_deferred
        FROM pg_constraint con
        JOIN pg_class c ON c.oid = con.conrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_class ref_class ON ref_class.oid = con.confrelid
        JOIN pg_namespace ref_ns ON ref_ns.oid = ref_class.relnamespace
        JOIN LATERAL (
            SELECT array_agg(att.attname ORDER BY keys.ordinality) AS names
            FROM unnest(con.conkey) WITH ORDINALITY AS keys(attnum, ordinality)
            JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = keys.attnum
        ) local_columns ON true
        JOIN LATERAL (
            SELECT array_agg(att.attname ORDER BY keys.ordinality) AS names
            FROM unnest(con.confkey) WITH ORDINALITY AS keys(attnum, ordinality)
            JOIN pg_attribute att ON att.attrelid = con.confrelid AND att.attnum = keys.attnum
        ) referenced_columns ON true
        WHERE n.nspname = $1
          AND c.relname = $2
          AND c.relkind IN ('r', 'p')
          AND con.contype = 'f'
        ORDER BY con.conname
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| TableForeignKey {
                name: row.name,
                columns: row.columns,
                referenced_schema: row.referenced_schema,
                referenced_table: row.referenced_table,
                referenced_columns: row.referenced_columns,
                on_update: ForeignKeyAction::from_postgres_code(&row.on_update),
                on_delete: ForeignKeyAction::from_postgres_code(&row.on_delete),
                is_deferrable: row.is_deferrable,
                is_initially_deferred: row.is_initially_deferred,
            })
            .collect()
    })
}
