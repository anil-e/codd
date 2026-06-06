use sqlx::PgPool;

use crate::db::object_actions::normalize_new_object_name;
use crate::models::database_object::quote_identifier;
use crate::models::structure_action::{
    StructureActionKind, StructureActionTarget, StructureDropMode,
};

pub async fn rename_structure_item(
    pool: &PgPool,
    target: &StructureActionTarget,
    new_name: &str,
) -> Result<(), sqlx::Error> {
    let Some(new_name) = normalize_new_object_name(new_name) else {
        return Err(sqlx::Error::Protocol(
            "Structure item name cannot be empty.".to_string(),
        ));
    };

    sqlx::query(&rename_structure_item_sql(target, &new_name)?)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn drop_structure_item(
    pool: &PgPool,
    target: &StructureActionTarget,
    mode: StructureDropMode,
) -> Result<(), sqlx::Error> {
    sqlx::query(&drop_structure_item_sql(target, mode)?)
        .execute(pool)
        .await?;

    Ok(())
}

fn rename_structure_item_sql(
    target: &StructureActionTarget,
    new_name: &str,
) -> Result<String, sqlx::Error> {
    ensure_editable(target)?;
    let Some(new_name) = normalize_new_object_name(new_name) else {
        return Err(sqlx::Error::Protocol(
            "Structure item name cannot be empty.".to_string(),
        ));
    };

    let table = target.table.qualified_name();
    let name = quote_identifier(&target.name);
    let new_name = quote_identifier(&new_name);

    let sql = match target.kind {
        StructureActionKind::Column => {
            format!("ALTER TABLE {table} RENAME COLUMN {name} TO {new_name}")
        }
        StructureActionKind::Constraint | StructureActionKind::ForeignKey => {
            format!("ALTER TABLE {table} RENAME CONSTRAINT {name} TO {new_name}")
        }
        StructureActionKind::Index => {
            format!("ALTER INDEX {} RENAME TO {new_name}", index_name(target))
        }
        StructureActionKind::Trigger => {
            format!("ALTER TRIGGER {name} ON {table} RENAME TO {new_name}")
        }
    };

    Ok(sql)
}

fn drop_structure_item_sql(
    target: &StructureActionTarget,
    mode: StructureDropMode,
) -> Result<String, sqlx::Error> {
    ensure_editable(target)?;

    let table = target.table.qualified_name();
    let name = quote_identifier(&target.name);
    let cascade = drop_mode_sql(mode);

    let sql = match target.kind {
        StructureActionKind::Column => {
            format!("ALTER TABLE {table} DROP COLUMN {name}{cascade}")
        }
        StructureActionKind::Constraint | StructureActionKind::ForeignKey => {
            format!("ALTER TABLE {table} DROP CONSTRAINT {name}{cascade}")
        }
        StructureActionKind::Index => {
            format!("DROP INDEX {}{cascade}", index_name(target))
        }
        StructureActionKind::Trigger => {
            format!("DROP TRIGGER {name} ON {table}{cascade}")
        }
    };

    Ok(sql)
}

fn ensure_editable(target: &StructureActionTarget) -> Result<(), sqlx::Error> {
    if target.editable {
        return Ok(());
    }

    Err(sqlx::Error::Protocol(
        "Structure item is read-only.".to_string(),
    ))
}

fn index_name(target: &StructureActionTarget) -> String {
    format!(
        "{}.{}",
        quote_identifier(&target.table.schema),
        quote_identifier(&target.name)
    )
}

fn drop_mode_sql(mode: StructureDropMode) -> &'static str {
    match mode {
        StructureDropMode::Restrict => "",
        StructureDropMode::Cascade => " CASCADE",
    }
}

#[cfg(test)]
mod tests {
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};

    use super::*;

    fn table() -> DatabaseObject {
        DatabaseObject {
            schema: "app schema".to_string(),
            name: "order\"line".to_string(),
            kind: DatabaseObjectKind::Table,
        }
    }

    fn target(kind: StructureActionKind, name: &str) -> StructureActionTarget {
        StructureActionTarget::new(table(), kind, name, true)
    }

    #[test]
    fn builds_column_sql() {
        let target = target(StructureActionKind::Column, "old name");

        assert_eq!(
            rename_structure_item_sql(&target, "new name").unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" RENAME COLUMN \"old name\" TO \"new name\""
        );
        assert_eq!(
            drop_structure_item_sql(&target, StructureDropMode::Restrict).unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" DROP COLUMN \"old name\""
        );
    }

    #[test]
    fn builds_constraint_sql() {
        let target = target(StructureActionKind::Constraint, "order check");

        assert_eq!(
            rename_structure_item_sql(&target, "renamed").unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" RENAME CONSTRAINT \"order check\" TO \"renamed\""
        );
        assert_eq!(
            drop_structure_item_sql(&target, StructureDropMode::Restrict).unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" DROP CONSTRAINT \"order check\""
        );
    }

    #[test]
    fn builds_foreign_key_sql_with_constraint_commands() {
        let target = target(StructureActionKind::ForeignKey, "customer fk");

        assert_eq!(
            rename_structure_item_sql(&target, "renamed").unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" RENAME CONSTRAINT \"customer fk\" TO \"renamed\""
        );
        assert_eq!(
            drop_structure_item_sql(&target, StructureDropMode::Restrict).unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" DROP CONSTRAINT \"customer fk\""
        );
    }

    #[test]
    fn builds_index_sql() {
        let target = target(StructureActionKind::Index, "line idx");

        assert_eq!(
            rename_structure_item_sql(&target, "renamed").unwrap(),
            "ALTER INDEX \"app schema\".\"line idx\" RENAME TO \"renamed\""
        );
        assert_eq!(
            drop_structure_item_sql(&target, StructureDropMode::Restrict).unwrap(),
            "DROP INDEX \"app schema\".\"line idx\""
        );
    }

    #[test]
    fn builds_trigger_sql() {
        let target = target(StructureActionKind::Trigger, "audit trigger");

        assert_eq!(
            rename_structure_item_sql(&target, "renamed").unwrap(),
            "ALTER TRIGGER \"audit trigger\" ON \"app schema\".\"order\"\"line\" RENAME TO \"renamed\""
        );
        assert_eq!(
            drop_structure_item_sql(&target, StructureDropMode::Restrict).unwrap(),
            "DROP TRIGGER \"audit trigger\" ON \"app schema\".\"order\"\"line\""
        );
    }

    #[test]
    fn read_only_targets_do_not_build_mutating_sql() {
        let target = StructureActionTarget::new(
            table(),
            StructureActionKind::Index,
            "primary key idx",
            false,
        );

        assert!(rename_structure_item_sql(&target, "renamed").is_err());
        assert!(drop_structure_item_sql(&target, StructureDropMode::Restrict).is_err());
    }

    #[test]
    fn builds_cascade_drop_sql() {
        let column = target(StructureActionKind::Column, "old name");
        let constraint = target(StructureActionKind::Constraint, "order check");
        let index = target(StructureActionKind::Index, "line idx");
        let trigger = target(StructureActionKind::Trigger, "audit trigger");

        assert_eq!(
            drop_structure_item_sql(&column, StructureDropMode::Cascade).unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" DROP COLUMN \"old name\" CASCADE"
        );
        assert_eq!(
            drop_structure_item_sql(&constraint, StructureDropMode::Cascade).unwrap(),
            "ALTER TABLE \"app schema\".\"order\"\"line\" DROP CONSTRAINT \"order check\" CASCADE"
        );
        assert_eq!(
            drop_structure_item_sql(&index, StructureDropMode::Cascade).unwrap(),
            "DROP INDEX \"app schema\".\"line idx\" CASCADE"
        );
        assert_eq!(
            drop_structure_item_sql(&trigger, StructureDropMode::Cascade).unwrap(),
            "DROP TRIGGER \"audit trigger\" ON \"app schema\".\"order\"\"line\" CASCADE"
        );
    }

    #[test]
    fn rejects_invalid_new_names() {
        let target = target(StructureActionKind::Column, "old name");

        assert!(rename_structure_item_sql(&target, "").is_err());
        assert!(rename_structure_item_sql(&target, " \t ").is_err());
        assert!(rename_structure_item_sql(&target, "bad\0name").is_err());
    }
}
