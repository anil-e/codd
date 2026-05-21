mod columns;
mod constraints;
mod foreign_keys;
mod indexes;
mod triggers;

use sqlx::PgPool;

use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_structure::TableStructure;

use columns::load_columns;
use constraints::load_constraints;
use foreign_keys::load_foreign_keys;
use indexes::load_indexes;
use triggers::load_triggers;

pub async fn load_table_structure(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<TableStructure, sqlx::Error> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(sqlx::Error::Protocol(
            "Structure can only be loaded for tables.".to_string(),
        ));
    }

    let columns = load_columns(pool, object).await?;
    let indexes = load_indexes(pool, object).await?;
    let constraints = load_constraints(pool, object).await?;
    let foreign_keys = load_foreign_keys(pool, object).await?;
    let triggers = load_triggers(pool, object).await?;

    Ok(TableStructure {
        object: object.clone(),
        columns,
        indexes,
        constraints,
        foreign_keys,
        triggers,
    })
}
