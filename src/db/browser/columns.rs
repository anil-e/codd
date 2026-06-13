use crate::models::database_object::DatabaseObject;
use crate::models::table_browser::{ColumnTypeGroup, TableColumn};
use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
struct TableColumnRow {
    name: String,
    display_type: String,
    type_name: String,
    enum_values: Option<Vec<String>>,
    is_nullable: bool,
    is_primary_key: bool,
    has_default: bool,
    is_identity: bool,
    is_generated: bool,
    ordinal_position: i32,
}

pub async fn load_table_columns(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableColumn>, sqlx::Error> {
    let rows = sqlx::query_as::<_, TableColumnRow>(
        r"
        SELECT
            a.attname AS name,
            format_type(a.atttypid, a.atttypmod) AS display_type,
            t.typname AS type_name,
            array_agg(e.enumlabel ORDER BY e.enumsortorder)
                FILTER (WHERE e.enumlabel IS NOT NULL) AS enum_values,
            NOT a.attnotnull AS is_nullable,
            EXISTS (
                SELECT 1
                FROM pg_constraint con
                WHERE con.conrelid = c.oid
                  AND con.contype = 'p'
                  AND a.attnum = ANY(con.conkey)
            ) AS is_primary_key,
            ad.oid IS NOT NULL AS has_default,
            a.attidentity <> '' AS is_identity,
            a.attgenerated <> '' AS is_generated,
            a.attnum::int4 AS ordinal_position
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_attribute a ON a.attrelid = c.oid
        JOIN pg_type t ON t.oid = a.atttypid
        LEFT JOIN pg_enum e ON e.enumtypid = t.oid
        LEFT JOIN pg_attrdef ad ON ad.adrelid = c.oid
            AND ad.adnum = a.attnum
        WHERE n.nspname = $1
          AND c.relname = $2
          AND a.attnum > 0
          AND NOT a.attisdropped
          AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
        GROUP BY
            c.oid,
            a.attname,
            a.atttypid,
            a.atttypmod,
            t.typname,
            a.attnotnull,
            ad.oid,
            a.attidentity,
            a.attgenerated,
            a.attnum
        ORDER BY a.attnum
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| TableColumn {
            name: row.name,
            display_type: row.display_type,
            enum_values: row.enum_values.unwrap_or_default(),
            type_group: ColumnTypeGroup::from_postgres_type(&row.type_name),
            type_name: row.type_name,
            is_nullable: row.is_nullable,
            is_primary_key: row.is_primary_key,
            has_default: row.has_default,
            is_identity: row.is_identity,
            is_generated: row.is_generated,
            ordinal_position: row.ordinal_position,
        })
        .collect())
}
