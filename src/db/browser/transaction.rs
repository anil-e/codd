use std::collections::HashSet;

use sqlx::{PgPool, Postgres, Transaction};

use super::editing::{delete_row_sql, validate_update_input};
use super::page_sql::{ROW_VERSION_SQL, primary_key_columns};
use crate::models::database_object::{DatabaseObject, quote_identifier};
use crate::models::table_browser::{TableColumn, TableRowEdit};

#[derive(Debug)]
pub enum TableEditError {
    Conflict { row_index: usize },
    Failed(String),
    CommitUnknown(String),
}

impl std::fmt::Display for TableEditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { row_index } => write!(
                formatter,
                "Row {} has changed. Reload before saving.",
                row_index.saturating_add(1)
            ),
            Self::Failed(message) => formatter.write_str(message),
            Self::CommitUnknown(message) => write!(
                formatter,
                "The commit outcome is unknown. Reload before retrying: {message}"
            ),
        }
    }
}

impl std::error::Error for TableEditError {}

impl From<sqlx::Error> for TableEditError {
    fn from(error: sqlx::Error) -> Self {
        Self::Failed(error.to_string())
    }
}

pub async fn apply_table_edits(
    pool: &PgPool,
    object: &DatabaseObject,
    columns: &[TableColumn],
    edits: &[TableRowEdit],
) -> Result<(), TableEditError> {
    validate_edits(object, columns, edits)?;

    if edits.is_empty() {
        return Ok(());
    }

    let mut transaction = pool.begin().await?;
    let result = apply_locked_edits(&mut transaction, object, columns, edits).await;

    if let Err(error) = result {
        transaction.rollback().await?;

        return Err(error);
    }

    transaction.commit().await.map_err(|error| match &error {
        sqlx::Error::Database(database)
            if database
                .code()
                .is_some_and(|code| code.starts_with("23") || code.starts_with("40")) =>
        {
            TableEditError::Failed(error.to_string())
        }
        _ => TableEditError::CommitUnknown(error.to_string()),
    })?;

    Ok(())
}

fn validate_edits(
    object: &DatabaseObject,
    columns: &[TableColumn],
    edits: &[TableRowEdit],
) -> Result<(), TableEditError> {
    if edits.len() > 500 || columns.len() > 1600 {
        return Err(TableEditError::Failed("Too many staged edits.".to_string()));
    }

    let mut identities = HashSet::new();
    let mut row_indices = HashSet::new();

    for edit in edits {
        if edit.version.is_empty() || edit.changes.is_empty() || edit.changes.len() > columns.len()
        {
            return Err(TableEditError::Failed("Invalid staged row.".to_string()));
        }

        let mut changed_columns = HashSet::new();

        for change in &edit.changes {
            validate_update_input(object, columns, &edit.original_row, change.column_index)
                .map_err(|error| TableEditError::Failed(error.to_string()))?;

            if !changed_columns.insert(change.column_index)
                || (change.value.is_null && !columns[change.column_index].is_nullable)
            {
                return Err(TableEditError::Failed("Invalid staged cell.".to_string()));
            }
        }

        let identity = primary_key_columns(columns)
            .iter()
            .map(|(index, _)| &edit.original_row[*index])
            .map(|cell| (!cell.is_null).then_some(cell.value.as_str()))
            .collect::<Vec<_>>();

        if identity.iter().any(Option::is_none)
            || !identities.insert(identity)
            || !row_indices.insert(edit.row_index)
        {
            return Err(TableEditError::Failed(
                "Duplicate or invalid row identity.".to_string(),
            ));
        }
    }

    Ok(())
}

#[derive(sqlx::FromRow)]
struct ColumnMetadata {
    name: String,
    display_type: String,
    is_primary_key: bool,
    is_nullable: bool,
    is_identity: bool,
    is_generated: bool,
    ordinal_position: i32,
}

async fn apply_locked_edits(
    transaction: &mut Transaction<'_, Postgres>,
    object: &DatabaseObject,
    columns: &[TableColumn],
    edits: &[TableRowEdit],
) -> Result<(), TableEditError> {
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut **transaction)
        .await?;

    sqlx::query("SET LOCAL statement_timeout = '30s'")
        .execute(&mut **transaction)
        .await?;

    sqlx::query(&format!(
        "LOCK TABLE {} IN ROW EXCLUSIVE MODE",
        object.qualified_name()
    ))
    .execute(&mut **transaction)
    .await?;

    let metadata = sqlx::query_as::<_, ColumnMetadata>(
        "SELECT a.attname AS name, format_type(a.atttypid, a.atttypmod) AS display_type,
         EXISTS (SELECT 1 FROM pg_constraint c WHERE c.conrelid = a.attrelid
             AND c.contype = 'p' AND a.attnum = ANY(c.conkey)) AS is_primary_key,
         NOT a.attnotnull AS is_nullable, a.attidentity <> '' AS is_identity,
         a.attgenerated <> '' AS is_generated, a.attnum::int4 AS ordinal_position
         FROM pg_attribute a WHERE a.attrelid = to_regclass($1)
             AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum",
    )
    .bind(object.qualified_name())
    .fetch_all(&mut **transaction)
    .await?;

    if metadata.len() != columns.len()
        || metadata.iter().zip(columns).any(|(actual, expected)| {
            actual.name != expected.name
                || actual.display_type != expected.display_type
                || actual.is_primary_key != expected.is_primary_key
                || actual.is_nullable != expected.is_nullable
                || actual.is_identity != expected.is_identity
                || actual.is_generated != expected.is_generated
                || actual.ordinal_position != expected.ordinal_position
        })
    {
        return Err(TableEditError::Failed(
            "The table definition has changed. Reload before saving.".to_string(),
        ));
    }

    let delete_sql = delete_row_sql(object, columns)
        .map_err(|error| TableEditError::Failed(error.to_string()))?;
    let predicate = delete_sql
        .strip_prefix(&format!("DELETE FROM {} WHERE ", object.qualified_name()))
        .ok_or_else(|| TableEditError::Failed("Invalid row predicate.".to_string()))?;
    let lock_sql = format!(
        "SELECT {ROW_VERSION_SQL} FROM {} WHERE {predicate} LIMIT 2 FOR UPDATE",
        object.qualified_name()
    );
    let mut locked_versions = HashSet::new();

    for edit in edits {
        let mut query = sqlx::query_scalar::<_, String>(&lock_sql);

        for (index, _) in primary_key_columns(columns) {
            query = query.bind(&edit.original_row[index].value);
        }

        let versions = query.fetch_all(&mut **transaction).await?;

        if versions.len() != 1 || versions[0] != edit.version {
            return Err(TableEditError::Conflict {
                row_index: edit.row_index,
            });
        }

        if !locked_versions.insert(versions[0].clone()) {
            return Err(TableEditError::Failed(
                "Duplicate row identity.".to_string(),
            ));
        }
    }

    for edit in edits {
        let assignments = edit
            .changes
            .iter()
            .enumerate()
            .map(|(index, change)| {
                let column = &columns[change.column_index];

                format!(
                    "{} = ${}::{}",
                    quote_identifier(&column.name),
                    index + 1,
                    column.display_type
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let key_predicate = primary_key_columns(columns)
            .iter()
            .enumerate()
            .map(|(index, (_, column))| {
                format!(
                    "{} IS NOT DISTINCT FROM ${}::{}",
                    quote_identifier(&column.name),
                    edit.changes.len() + index + 1,
                    column.display_type
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!(
            "UPDATE {} SET {assignments} WHERE {key_predicate} AND {ROW_VERSION_SQL} = ${}::text",
            object.qualified_name(),
            edit.changes.len() + primary_key_columns(columns).len() + 1
        );
        let mut query = sqlx::query(&sql);

        for change in &edit.changes {
            query = query.bind((!change.value.is_null).then_some(change.value.value.as_str()));
        }

        for (index, _) in primary_key_columns(columns) {
            query = query.bind(&edit.original_row[index].value);
        }

        let result = query
            .bind(&edit.version)
            .execute(&mut **transaction)
            .await?;

        if result.rows_affected() != 1 {
            return Err(TableEditError::Conflict {
                row_index: edit.row_index,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::browser::load_table_page;
    use crate::models::database_object::DatabaseObjectKind;
    use crate::models::table_browser::{TableCell, TableCellChange, TablePage};

    fn edit(page: &TablePage, row_index: usize, values: &[(usize, &str)]) -> TableRowEdit {
        TableRowEdit {
            row_index,
            original_row: page.rows[row_index].clone(),
            version: page.row_versions[row_index].clone(),
            changes: values
                .iter()
                .map(|(column_index, value)| TableCellChange {
                    column_index: *column_index,
                    value: TableCell::new((*value).to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn commit_errors_are_not_implicitly_retryable() {
        let error = TableEditError::CommitUnknown("connection lost".to_string());

        assert!(error.to_string().contains("Reload before retrying"));
        assert!(
            TableEditError::Conflict { row_index: 0 }
                .to_string()
                .starts_with("Row 1 ")
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database with CREATEROLE"]
    async fn column_select_grants_allow_read_only_browsing() {
        let url = std::env::var("DATABASE_URL").unwrap();
        let pool = PgPool::connect(&url).await.unwrap();
        let schema = format!("column_grants_{}", uuid::Uuid::new_v4().simple());
        let role = format!("reader_{}", uuid::Uuid::new_v4().simple());
        let quoted_role = quote_identifier(&role);
        let object = DatabaseObject {
            schema: schema.clone(),
            name: "rows".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let table = object.qualified_name();

        sqlx::query(&format!("CREATE ROLE {quoted_role}"))
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(&format!("CREATE SCHEMA {}", quote_identifier(&schema)))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(&format!(
            "CREATE TABLE {table} (id integer PRIMARY KEY, value text)"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {table} VALUES (1, 'first'), (2, 'second'), (3, 'third')"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "GRANT USAGE ON SCHEMA {} TO {quoted_role}",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT (id, value) ON {table} TO {quoted_role}"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let set_role = format!("SET ROLE {quoted_role}");
        let reader = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |connection, _| {
                let set_role = set_role.clone();
                Box::pin(async move {
                    sqlx::query(&set_role).execute(connection).await?;

                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();

        let error = sqlx::query(&format!("SELECT {ROW_VERSION_SQL}, id, value FROM {table}"))
            .fetch_all(&reader)
            .await
            .unwrap_err();

        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );

        let filters = [crate::models::table_browser::TableFilter::column(
            "id".to_string(),
            crate::models::table_browser::FilterOperator::GreaterThan,
            Some("1".to_string()),
        )];

        let page = load_table_page(&reader, &object, 0, 1, &filters, None)
            .await
            .unwrap();

        assert_eq!(
            page.rows,
            vec![vec![
                TableCell::new("2".to_string()),
                TableCell::new("second".to_string())
            ]]
        );
        assert!(page.has_next_page);
        assert!(page.row_versions.is_empty());

        let staged = TableRowEdit {
            row_index: 0,
            original_row: page.rows[0].clone(),
            version: String::new(),
            changes: vec![TableCellChange {
                column_index: 1,
                value: TableCell::new("changed".to_string()),
            }],
        };

        assert!(matches!(
            apply_table_edits(&reader, &object, &page.columns, &[staged]).await,
            Err(TableEditError::Failed(_))
        ));

        sqlx::query(&format!(
            "REVOKE SELECT (value) ON {table} FROM {quoted_role}"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let error = load_table_page(&reader, &object, 0, 1, &filters, None)
            .await
            .unwrap_err();

        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );

        reader.close().await;

        sqlx::query(&format!(
            "DROP SCHEMA {} CASCADE",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(&format!("DROP ROLE {quoted_role}"))
            .execute(&pool)
            .await
            .unwrap();

        pool.close().await;
    }

    #[test]
    fn version_projection_does_not_change_visible_columns() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "rows".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        let query = super::super::page_sql::table_page_sql_with_filters(
            &object,
            &[],
            0,
            10,
            &[],
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            query.sql,
            format!(
                "SELECT {ROW_VERSION_SQL}, * FROM \"public\".\"rows\" ORDER BY ctid LIMIT 11 OFFSET 0"
            )
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing to a disposable PostgreSQL database"]
    async fn staged_updates_are_atomic_and_detect_conflicts() {
        let pool = PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
            .await
            .unwrap();
        let schema = format!("staged_{}", uuid::Uuid::new_v4().simple());
        let object = DatabaseObject {
            schema: schema.clone(),
            name: "quoted \" rows".to_string(),
            kind: DatabaseObjectKind::Table,
        };

        sqlx::query(&format!("CREATE SCHEMA {}", quote_identifier(&schema)))
            .execute(&pool)
            .await
            .unwrap();

        let table = object.qualified_name();

        sqlx::query(&format!("CREATE TABLE {table} (\"tenant id\" integer, \"key\" integer, a integer, b integer, PRIMARY KEY (\"tenant id\", \"key\"), CHECK (a <= b), UNIQUE (a) DEFERRABLE INITIALLY DEFERRED, UNIQUE (b))"))
            .execute(&pool).await.unwrap();
        sqlx::query(&format!(
            "INSERT INTO {table} VALUES (1, 1, 1, 2), (1, 2, 3, 4)"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let page = load_table_page(&pool, &object, 0, 100, &[], None)
            .await
            .unwrap();

        assert_eq!(page.rows[0].len(), 4);
        assert_eq!(page.row_versions.len(), 2);

        let valid = edit(&page, 0, &[(2, "5"), (3, "6")]);
        let invalid = edit(&page, 1, &[(2, "9")]);

        assert!(matches!(
            apply_table_edits(&pool, &object, &page.columns, &[valid.clone(), invalid]).await,
            Err(TableEditError::Failed(_))
        ));
        assert_eq!(
            load_table_page(&pool, &object, 0, 100, &[], None)
                .await
                .unwrap()
                .rows,
            page.rows
        );
        assert!(
            apply_table_edits(
                &pool,
                &object,
                &page.columns,
                &[valid.clone(), valid.clone()]
            )
            .await
            .is_err()
        );

        let mut duplicate_cell = valid.clone();
        duplicate_cell
            .changes
            .push(duplicate_cell.changes[0].clone());

        assert!(validate_edits(&object, &page.columns, &[duplicate_cell]).is_err());

        let mut primary_key = valid.clone();
        primary_key.changes[0].column_index = 0;

        assert!(validate_edits(&object, &page.columns, &[primary_key]).is_err());

        let mut generated_columns = page.columns.clone();
        generated_columns[2].is_generated = true;

        assert!(validate_edits(&object, &generated_columns, std::slice::from_ref(&valid)).is_err());

        generated_columns[2].is_generated = false;
        generated_columns[2].is_identity = true;

        assert!(validate_edits(&object, &generated_columns, std::slice::from_ref(&valid)).is_err());
        assert!(validate_edits(&object, &page.columns, &vec![valid.clone(); 501]).is_err());

        apply_table_edits(&pool, &object, &page.columns, std::slice::from_ref(&valid))
            .await
            .unwrap();

        assert!(matches!(
            apply_table_edits(&pool, &object, &page.columns, &[valid]).await,
            Err(TableEditError::Conflict { row_index: 0 })
        ));

        let page = load_table_page(&pool, &object, 0, 100, &[], None)
            .await
            .unwrap();
        let deferred = edit(&page, 1, &[(2, "5"), (3, "7")]);

        assert!(matches!(
            apply_table_edits(&pool, &object, &page.columns, &[deferred]).await,
            Err(TableEditError::Failed(_))
        ));

        let unique = edit(&page, 1, &[(3, "6")]);

        assert!(matches!(
            apply_table_edits(&pool, &object, &page.columns, &[unique]).await,
            Err(TableEditError::Failed(_))
        ));
        assert_eq!(
            load_table_page(&pool, &object, 0, 100, &[], None)
                .await
                .unwrap()
                .rows,
            page.rows
        );

        sqlx::query(&format!("UPDATE {table} SET b = 8 WHERE \"key\" = 2"))
            .execute(&pool)
            .await
            .unwrap();
        let external = edit(&page, 1, &[(3, "9")]);

        assert!(matches!(
            apply_table_edits(&pool, &object, &page.columns, &[external]).await,
            Err(TableEditError::Conflict { row_index: 1 })
        ));

        let page = load_table_page(&pool, &object, 0, 100, &[], None)
            .await
            .unwrap();

        sqlx::query(&format!("DELETE FROM {table} WHERE \"key\" = 2"))
            .execute(&pool)
            .await
            .unwrap();

        let deleted = edit(&page, 1, &[(3, "9")]);

        assert!(matches!(
            apply_table_edits(
                &pool,
                &object,
                &page.columns,
                std::slice::from_ref(&deleted)
            )
            .await,
            Err(TableEditError::Conflict { row_index: 1 })
        ));

        sqlx::query(&format!("INSERT INTO {table} VALUES (1, 2, 3, 8)"))
            .execute(&pool)
            .await
            .unwrap();

        assert!(matches!(
            apply_table_edits(&pool, &object, &page.columns, &[deleted]).await,
            Err(TableEditError::Conflict { row_index: 1 })
        ));

        sqlx::query(&format!("CREATE FUNCTION {}.suppress_update() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RETURN NULL; END'", quote_identifier(&schema)))
            .execute(&pool).await.unwrap();
        sqlx::query(&format!("CREATE TRIGGER suppress_update BEFORE UPDATE ON {table} FOR EACH ROW WHEN (OLD.\"key\" = 2) EXECUTE FUNCTION {}.suppress_update()", quote_identifier(&schema)))
            .execute(&pool).await.unwrap();
        let current = load_table_page(&pool, &object, 0, 100, &[], None)
            .await
            .unwrap();

        assert!(matches!(
            apply_table_edits(
                &pool,
                &object,
                &current.columns,
                &[
                    edit(&current, 0, &[(3, "10")]),
                    edit(&current, 1, &[(3, "11")]),
                ]
            )
            .await,
            Err(TableEditError::Conflict { row_index: 1 })
        ));
        assert_eq!(
            load_table_page(&pool, &object, 0, 100, &[], None)
                .await
                .unwrap()
                .rows,
            current.rows
        );

        let partitioned = DatabaseObject {
            name: "partitioned".to_string(),
            ..object.clone()
        };
        let parent = partitioned.qualified_name();

        sqlx::query(&format!(
            "CREATE TABLE {parent} (id integer PRIMARY KEY, value text) PARTITION BY RANGE (id)"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TABLE {}.part1 PARTITION OF {parent} FOR VALUES FROM (0) TO (10)",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!("INSERT INTO {parent} VALUES (1, 'original')"))
            .execute(&pool)
            .await
            .unwrap();
        let partition_page = load_table_page(&pool, &partitioned, 0, 100, &[], None)
            .await
            .unwrap();

        sqlx::query(&format!(
            "ALTER TABLE {parent} DETACH PARTITION {}.part1",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TABLE {}.part2 PARTITION OF {parent} FOR VALUES FROM (0) TO (10)",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(&format!("INSERT INTO {parent} VALUES (1, 'original')"))
            .execute(&pool)
            .await
            .unwrap();

        assert!(matches!(
            apply_table_edits(
                &pool,
                &partitioned,
                &partition_page.columns,
                &[edit(&partition_page, 0, &[(1, "changed")])]
            )
            .await,
            Err(TableEditError::Conflict { row_index: 0 })
        ));

        sqlx::query(&format!(
            "CREATE VIEW {}.visible AS SELECT * FROM {table}",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();
        let view = DatabaseObject {
            name: "visible".to_string(),
            kind: DatabaseObjectKind::View,
            ..object.clone()
        };

        assert!(
            load_table_page(&pool, &view, 0, 1, &[], None)
                .await
                .unwrap()
                .row_versions
                .is_empty()
        );

        sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN extra text"))
            .execute(&pool)
            .await
            .unwrap();

        assert!(matches!(
            apply_table_edits(
                &pool,
                &object,
                &page.columns,
                &[edit(&page, 0, &[(3, "10")])]
            )
            .await,
            Err(TableEditError::Failed(_))
        ));

        sqlx::query(&format!(
            "DROP SCHEMA {} CASCADE",
            quote_identifier(&schema)
        ))
        .execute(&pool)
        .await
        .unwrap();

        pool.close().await;
    }
}
