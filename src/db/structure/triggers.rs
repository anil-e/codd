use sqlx::PgPool;

use crate::models::database_object::DatabaseObject;
use crate::models::table_structure::{TableTrigger, TriggerEnabledState};

#[derive(Debug, sqlx::FromRow)]
struct TableTriggerRow {
    name: String,
    definition: String,
    enabled: String,
    function_schema: String,
    function_name: String,
}

pub(super) async fn load_triggers(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<TableTrigger>, sqlx::Error> {
    sqlx::query_as::<_, TableTriggerRow>(
        r"
        SELECT
            t.tgname AS name,
            pg_get_triggerdef(t.oid, true) AS definition,
            t.tgenabled::text AS enabled,
            proc_ns.nspname AS function_schema,
            proc.proname AS function_name
        FROM pg_trigger t
        JOIN pg_class c ON c.oid = t.tgrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        JOIN pg_proc proc ON proc.oid = t.tgfoid
        JOIN pg_namespace proc_ns ON proc_ns.oid = proc.pronamespace
        WHERE n.nspname = $1
          AND c.relname = $2
          AND c.relkind IN ('r', 'p')
          AND NOT t.tgisinternal
        ORDER BY t.tgname
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| TableTrigger {
                name: row.name,
                definition: row.definition,
                enabled: TriggerEnabledState::from_postgres_code(&row.enabled),
                function_schema: row.function_schema,
                function_name: row.function_name,
            })
            .collect()
    })
}
