use sqlx::PgPool;

use crate::models::database_object::DatabaseObject;
use crate::models::table_structure::{TableConstraint, TableConstraintKind};

#[derive(Debug, sqlx::FromRow)]
struct TableConstraintRow {
    name: String,
    kind: String,
    definition: String,
    is_validated: bool,
    is_deferrable: bool,
    is_initially_deferred: bool,
}

pub(super) async fn load_constraints(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableConstraint>, sqlx::Error> {
    sqlx::query_as::<_, TableConstraintRow>(
        r"
        SELECT
            con.conname AS name,
            con.contype::text AS kind,
            pg_get_constraintdef(con.oid, true) AS definition,
            con.convalidated AS is_validated,
            con.condeferrable AS is_deferrable,
            con.condeferred AS is_initially_deferred
        FROM pg_constraint con
        JOIN pg_class c ON c.oid = con.conrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = $1
          AND c.relname = $2
          AND c.relkind IN ('r', 'p')
          AND con.contype <> 'f'
        ORDER BY
            CASE con.contype
                WHEN 'p' THEN 0
                WHEN 'u' THEN 1
                WHEN 'c' THEN 2
                ELSE 3
            END,
            con.conname
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| TableConstraint {
                name: row.name,
                kind: TableConstraintKind::from_postgres_code(&row.kind),
                definition: row.definition,
                is_validated: row.is_validated,
                is_deferrable: row.is_deferrable,
                is_initially_deferred: row.is_initially_deferred,
            })
            .collect()
    })
}
