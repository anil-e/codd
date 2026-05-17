use crate::models::database_object::{DatabaseObject, DatabaseObjectKind, quote_identifier};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TruncateOptions {
    pub restart_identity: bool,
    pub cascade: bool,
}

pub async fn rename_object(
    pool: &PgPool,
    object: &DatabaseObject,
    new_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(&rename_sql(object, new_name))
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn truncate_table(
    pool: &PgPool,
    object: &DatabaseObject,
    options: TruncateOptions,
) -> Result<(), sqlx::Error> {
    sqlx::query(&truncate_table_sql(object, options))
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn drop_object(pool: &PgPool, object: &DatabaseObject) -> Result<(), sqlx::Error> {
    sqlx::query(&drop_sql(object)).execute(pool).await?;

    Ok(())
}

fn rename_sql(object: &DatabaseObject, new_name: &str) -> String {
    let object_type = match object.kind {
        DatabaseObjectKind::Table => "TABLE",
        DatabaseObjectKind::View => "VIEW",
    };

    format!(
        "ALTER {object_type} {} RENAME TO {}",
        object.qualified_name(),
        quote_identifier(new_name)
    )
}

fn truncate_table_sql(object: &DatabaseObject, options: TruncateOptions) -> String {
    let mut sql = format!("TRUNCATE TABLE {}", object.qualified_name());

    if options.restart_identity {
        sql.push_str(" RESTART IDENTITY");
    }

    if options.cascade {
        sql.push_str(" CASCADE");
    }

    sql
}

fn drop_sql(object: &DatabaseObject) -> String {
    let object_type = match object.kind {
        DatabaseObjectKind::Table => "TABLE",
        DatabaseObjectKind::View => "VIEW",
    };

    format!("DROP {object_type} {}", object.qualified_name())
}

pub fn normalize_new_object_name(name: &str) -> Option<String> {
    let name = name.trim();

    if name.is_empty() || name.contains('\0') {
        return None;
    }

    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> DatabaseObject {
        DatabaseObject {
            schema: "public".to_string(),
            name: "customer data".to_string(),
            kind: DatabaseObjectKind::Table,
        }
    }

    fn view() -> DatabaseObject {
        DatabaseObject {
            schema: "reporting".to_string(),
            name: "active\"customers".to_string(),
            kind: DatabaseObjectKind::View,
        }
    }

    #[test]
    fn builds_quoted_table_rename_sql() {
        assert_eq!(
            rename_sql(&table(), "new name"),
            "ALTER TABLE \"public\".\"customer data\" RENAME TO \"new name\""
        );
    }

    #[test]
    fn builds_quoted_view_rename_sql() {
        assert_eq!(
            rename_sql(&view(), "current\"customers"),
            "ALTER VIEW \"reporting\".\"active\"\"customers\" RENAME TO \"current\"\"customers\""
        );
    }

    #[test]
    fn builds_truncate_sql() {
        assert_eq!(
            truncate_table_sql(&table(), TruncateOptions::default()),
            "TRUNCATE TABLE \"public\".\"customer data\""
        );
    }

    #[test]
    fn builds_truncate_sql_with_options() {
        assert_eq!(
            truncate_table_sql(
                &table(),
                TruncateOptions {
                    restart_identity: true,
                    cascade: true,
                },
            ),
            "TRUNCATE TABLE \"public\".\"customer data\" RESTART IDENTITY CASCADE"
        );
    }

    #[test]
    fn builds_drop_sql_for_tables_and_views() {
        assert_eq!(
            drop_sql(&table()),
            "DROP TABLE \"public\".\"customer data\""
        );
        assert_eq!(
            drop_sql(&view()),
            "DROP VIEW \"reporting\".\"active\"\"customers\""
        );
    }

    #[test]
    fn validates_new_object_name() {
        assert_eq!(normalize_new_object_name("  renamed  ").unwrap(), "renamed");
        assert!(normalize_new_object_name("").is_none());
        assert!(normalize_new_object_name(" \t ").is_none());
        assert!(normalize_new_object_name("bad\0name").is_none());
    }
}
