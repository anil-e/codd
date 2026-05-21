use sqlx::PgPool;

use crate::models::database_object::DatabaseObject;
use crate::models::table_structure::{TableColumnIdentity, TableStructureColumn};

#[derive(Debug, sqlx::FromRow)]
struct TableStructureColumnRow {
    name: String,
    data_type: String,
    type_name: String,
    is_nullable: bool,
    default_expression: Option<String>,
    is_primary_key: bool,
    identity: String,
    generated: String,
}

pub(super) async fn load_columns(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableStructureColumn>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TableStructureColumnRow>(
        r"
        SELECT
            a.attname AS name,
            format_type(a.atttypid, a.atttypmod) AS data_type,
            t.typname AS type_name,
            NOT a.attnotnull AS is_nullable,
            pg_get_expr(d.adbin, d.adrelid) AS default_expression,
            EXISTS (
                SELECT 1
                FROM pg_constraint con
                WHERE con.conrelid = c.oid
                  AND con.contype = 'p'
                  AND a.attnum = ANY(con.conkey)
            ) AS is_primary_key,
            a.attidentity::text AS identity,
            a.attgenerated::text AS generated
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_attribute a ON a.attrelid = c.oid
        JOIN pg_type t ON t.oid = a.atttypid
        LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
        WHERE n.nspname = $1
          AND c.relname = $2
          AND a.attnum > 0
          AND NOT a.attisdropped
          AND c.relkind IN ('r', 'p')
        ORDER BY a.attnum
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(table_structure_column).collect())
}

fn table_structure_column(row: TableStructureColumnRow) -> TableStructureColumn {
    TableStructureColumn {
        name: row.name,
        data_type: row.data_type,
        type_name: row.type_name,
        is_nullable: row.is_nullable,
        default_expression: row.default_expression,
        is_primary_key: row.is_primary_key,
        identity: TableColumnIdentity::from_postgres_identity(&row.identity),
        generated: if row.generated.is_empty() {
            None
        } else {
            Some(row.generated)
        },
    }
}
