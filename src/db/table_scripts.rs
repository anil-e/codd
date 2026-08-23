use crate::db::browser;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_browser::TableColumn;
use crate::models::table_script::TableScriptKind;
use sqlx::PgPool;

const TABLE_DEFINITION_SQL: &str = r"
SELECT
    pg_get_userbyid(c.relowner) AS owner,
    t.spcname AS tablespace
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_tablespace t ON t.oid = c.reltablespace
WHERE n.nspname = $1
  AND c.relname = $2
  AND c.relkind IN ('r', 'p')
";

const COLUMN_DEFINITION_SQL: &str = r"
SELECT
    a.attname AS name,
    format_type(a.atttypid, a.atttypmod) AS data_type,
    CASE
        WHEN a.attcollation <> 0
        THEN quote_ident(coll_ns.nspname) || '.' || quote_ident(coll.collname)
        ELSE NULL
    END AS collation,
    a.attnotnull AS is_not_null,
    pg_get_expr(d.adbin, d.adrelid) AS default_expression,
    a.attidentity::text AS identity,
    seq.seqincrement AS identity_increment,
    seq.seqstart AS identity_start,
    seq.seqmin AS identity_min_value,
    seq.seqmax AS identity_max_value,
    seq.seqcache AS identity_cache,
    seq.seqcycle AS identity_cycle,
    a.attgenerated::text AS generated
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
JOIN pg_attribute a ON a.attrelid = c.oid
LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
LEFT JOIN pg_collation coll ON coll.oid = a.attcollation
LEFT JOIN pg_namespace coll_ns ON coll_ns.oid = coll.collnamespace
LEFT JOIN pg_depend seq_dep
    ON seq_dep.refobjid = a.attrelid
   AND seq_dep.refobjsubid = a.attnum
   AND seq_dep.deptype = 'i'
LEFT JOIN pg_class seq_class
    ON seq_class.oid = seq_dep.objid
   AND seq_class.relkind = 'S'
LEFT JOIN pg_sequence seq ON seq.seqrelid = seq_class.oid
WHERE n.nspname = $1
  AND c.relname = $2
  AND a.attnum > 0
  AND NOT a.attisdropped
  AND c.relkind IN ('r', 'p')
ORDER BY a.attnum
";

pub async fn generate_table_script(
    pool: &PgPool,
    object: &DatabaseObject,
    kind: TableScriptKind,
) -> Result<String, sqlx::Error> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(sqlx::Error::Protocol(
            "Scripts can only be generated for tables.".to_string(),
        ));
    }

    match kind {
        TableScriptKind::Create => create_script(pool, object).await,
        TableScriptKind::Select => {
            let columns = browser::load_table_columns(pool, object).await?;
            Ok(select_script(object, &columns))
        }
        TableScriptKind::Insert => {
            let columns = browser::load_table_columns(pool, object).await?;
            Ok(insert_script(object, &columns))
        }
        TableScriptKind::Update => {
            let columns = browser::load_table_columns(pool, object).await?;
            Ok(update_script(object, &columns))
        }
        TableScriptKind::Delete => Ok(delete_script(object)),
    }
}

async fn create_script(pool: &PgPool, object: &DatabaseObject) -> Result<String, sqlx::Error> {
    let table = load_table_definition(pool, object).await?;
    let columns = load_column_definitions(pool, object).await?;
    let constraints = load_constraints(pool, object).await?;
    let indexes = load_indexes(pool, object).await?;

    Ok(build_create_script(
        object,
        &table,
        &columns,
        &constraints,
        &indexes,
    ))
}

#[derive(Debug, sqlx::FromRow)]
struct TableDefinition {
    owner: String,
    tablespace: Option<String>,
}

#[derive(Debug)]
struct ColumnDefinition {
    name: String,
    data_type: String,
    collation: Option<String>,
    is_not_null: bool,
    default_expression: Option<String>,
    identity: String,
    identity_sequence: Option<IdentitySequenceOptions>,
    generated: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ColumnDefinitionRow {
    name: String,
    data_type: String,
    collation: Option<String>,
    is_not_null: bool,
    default_expression: Option<String>,
    identity: String,
    identity_increment: Option<i64>,
    identity_start: Option<i64>,
    identity_min_value: Option<i64>,
    identity_max_value: Option<i64>,
    identity_cache: Option<i64>,
    identity_cycle: Option<bool>,
    generated: String,
}

#[derive(Debug)]
struct IdentitySequenceOptions {
    increment: i64,
    start: i64,
    min_value: i64,
    max_value: i64,
    cache: i64,
    cycle: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct ConstraintDefinition {
    name: String,
    definition: String,
}

#[derive(Debug, sqlx::FromRow)]
struct IndexDefinition {
    schema: String,
    name: String,
    definition: String,
}

async fn load_table_definition(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<TableDefinition, sqlx::Error> {
    sqlx::query_as::<_, TableDefinition>(TABLE_DEFINITION_SQL)
        .bind(&object.schema)
        .bind(&object.name)
        .fetch_one(pool)
        .await
}

async fn load_column_definitions(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<ColumnDefinition>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ColumnDefinitionRow>(COLUMN_DEFINITION_SQL)
        .bind(&object.schema)
        .bind(&object.name)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let identity_sequence = identity_sequence_options(
                row.identity_increment,
                row.identity_start,
                row.identity_min_value,
                row.identity_max_value,
                row.identity_cache,
                row.identity_cycle,
            );

            ColumnDefinition {
                name: row.name,
                data_type: row.data_type,
                collation: row.collation,
                is_not_null: row.is_not_null,
                default_expression: row.default_expression,
                identity: row.identity,
                identity_sequence,
                generated: row.generated,
            }
        })
        .collect())
}

async fn load_constraints(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<ConstraintDefinition>, sqlx::Error> {
    sqlx::query_as::<_, ConstraintDefinition>(
        r"
        SELECT
            con.conname AS name,
            pg_get_constraintdef(con.oid, true) AS definition
        FROM pg_constraint con
        JOIN pg_class c ON c.oid = con.conrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = $1
          AND c.relname = $2
          AND con.contype IN ('p', 'u', 'f', 'c', 'x')
        ORDER BY
            CASE con.contype
                WHEN 'p' THEN 0
                WHEN 'u' THEN 1
                WHEN 'f' THEN 2
                WHEN 'c' THEN 3
                ELSE 4
            END,
            con.conname
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await
}

async fn load_indexes(
    pool: &PgPool,
    object: &DatabaseObject,
) -> Result<Vec<IndexDefinition>, sqlx::Error> {
    sqlx::query_as::<_, IndexDefinition>(
        r"
        SELECT
            index_ns.nspname AS schema,
            index_class.relname AS name,
            pg_get_indexdef(index_class.oid) AS definition
        FROM pg_index idx
        JOIN pg_class table_class ON table_class.oid = idx.indrelid
        JOIN pg_namespace table_ns ON table_ns.oid = table_class.relnamespace
        JOIN pg_class index_class ON index_class.oid = idx.indexrelid
        JOIN pg_namespace index_ns ON index_ns.oid = index_class.relnamespace
        WHERE table_ns.nspname = $1
          AND table_class.relname = $2
          AND NOT idx.indisprimary
          AND NOT EXISTS (
              SELECT 1
              FROM pg_constraint con
              WHERE con.conindid = idx.indexrelid
          )
        ORDER BY index_class.relname
        ",
    )
    .bind(&object.schema)
    .bind(&object.name)
    .fetch_all(pool)
    .await
}

fn identity_sequence_options(
    increment: Option<i64>,
    start: Option<i64>,
    min_value: Option<i64>,
    max_value: Option<i64>,
    cache: Option<i64>,
    cycle: Option<bool>,
) -> Option<IdentitySequenceOptions> {
    Some(IdentitySequenceOptions {
        increment: increment?,
        start: start?,
        min_value: min_value?,
        max_value: max_value?,
        cache: cache?,
        cycle: cycle?,
    })
}

fn build_create_script(
    object: &DatabaseObject,
    table: &TableDefinition,
    columns: &[ColumnDefinition],
    constraints: &[ConstraintDefinition],
    indexes: &[IndexDefinition],
) -> String {
    let table_name = qualified_name(&object.schema, &object.name);
    let mut script = String::new();

    script.push_str(&format!("-- Table: {table_name}\n\n"));
    script.push_str(&format!("-- DROP TABLE IF EXISTS {table_name};\n\n"));
    script.push_str(&format!("CREATE TABLE IF NOT EXISTS {table_name}\n(\n"));

    let definitions = columns
        .iter()
        .map(column_sql)
        .chain(constraints.iter().map(constraint_sql))
        .collect::<Vec<_>>();

    script.push_str(&definitions.join(",\n"));
    script.push_str("\n)");

    if let Some(tablespace) = table.tablespace.as_deref() {
        script.push_str(&format!("\n\nTABLESPACE {};", identifier(tablespace)));
    } else {
        script.push(';');
    }

    script.push_str("\n\n");
    script.push_str(&format!(
        "ALTER TABLE IF EXISTS {table_name}\n    OWNER to {};",
        identifier(&table.owner)
    ));

    for index in indexes {
        script.push_str("\n\n");
        script.push_str(&format!(
            "-- Index: {}.{}\n\n",
            identifier(&index.schema),
            identifier(&index.name)
        ));
        script.push_str(&format!(
            "-- DROP INDEX IF EXISTS {}.{};\n\n",
            identifier(&index.schema),
            identifier(&index.name)
        ));
        script.push_str(&create_index_if_not_exists(&index.definition));
        script.push(';');
    }

    script
}

fn column_sql(column: &ColumnDefinition) -> String {
    let mut sql = format!("    {} {}", identifier(&column.name), column.data_type);

    if let Some(collation) = column.collation.as_deref() {
        sql.push_str(&format!(" COLLATE {collation}"));
    }

    if column.is_not_null {
        sql.push_str(" NOT NULL");
    }

    if !column.identity.is_empty() {
        let identity = match column.identity.as_str() {
            "a" => "ALWAYS",
            "d" => "BY DEFAULT",
            _ => "",
        };

        if !identity.is_empty() {
            sql.push_str(&format!(" GENERATED {identity} AS IDENTITY"));

            if let Some(options) = column.identity_sequence.as_ref() {
                sql.push_str(&identity_sequence_options_sql(options));
            }
        }
    } else if !column.generated.is_empty() {
        if let Some(expression) = column.default_expression.as_deref() {
            sql.push_str(&format!(" GENERATED ALWAYS AS ({expression}) STORED"));
        }
    } else if let Some(default_expression) = column.default_expression.as_deref() {
        sql.push_str(&format!(" DEFAULT {default_expression}"));
    }

    sql
}

fn constraint_sql(constraint: &ConstraintDefinition) -> String {
    format!(
        "    CONSTRAINT {} {}",
        identifier(&constraint.name),
        constraint.definition
    )
}

fn select_script(object: &DatabaseObject, columns: &[TableColumn]) -> String {
    let columns = column_list(columns);

    format!(
        "SELECT {columns}\n    FROM {};",
        qualified_name(&object.schema, &object.name)
    )
}

fn insert_script(object: &DatabaseObject, columns: &[TableColumn]) -> String {
    let column_count = columns.len();
    let columns = column_list(columns);
    let placeholders = placeholders(column_count);

    format!(
        "INSERT INTO {}(\n    {columns})\n    VALUES ({placeholders});",
        qualified_name(&object.schema, &object.name)
    )
}

fn update_script(object: &DatabaseObject, columns: &[TableColumn]) -> String {
    let assignments = columns
        .iter()
        .enumerate()
        .map(|(index, column)| format!("{} = ${}", identifier(&column.name), index + 1))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "UPDATE {}\n    SET {assignments}\n    WHERE <condition>;",
        qualified_name(&object.schema, &object.name)
    )
}

fn delete_script(object: &DatabaseObject) -> String {
    format!(
        "DELETE FROM {}\n    WHERE <condition>;",
        qualified_name(&object.schema, &object.name)
    )
}

fn column_list(columns: &[TableColumn]) -> String {
    if columns.is_empty() {
        return "*".to_string();
    }

    columns
        .iter()
        .map(|column| identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn qualified_name(schema: &str, name: &str) -> String {
    format!("{}.{}", identifier(schema), identifier(name))
}

fn identifier(value: &str) -> String {
    if is_simple_identifier(value) && !is_reserved_identifier(value) {
        return value.to_string();
    }

    format!("\"{}\"", value.replace('"', "\"\""))
}

fn is_simple_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first.is_ascii_lowercase() || first == '_')
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn is_reserved_identifier(value: &str) -> bool {
    matches!(
        value,
        "all"
            | "analyse"
            | "analyze"
            | "and"
            | "any"
            | "array"
            | "as"
            | "asc"
            | "both"
            | "case"
            | "cast"
            | "check"
            | "collate"
            | "column"
            | "constraint"
            | "create"
            | "current_catalog"
            | "current_date"
            | "current_role"
            | "current_time"
            | "current_timestamp"
            | "current_user"
            | "default"
            | "delete"
            | "desc"
            | "distinct"
            | "do"
            | "else"
            | "end"
            | "except"
            | "false"
            | "fetch"
            | "for"
            | "foreign"
            | "from"
            | "grant"
            | "group"
            | "having"
            | "in"
            | "insert"
            | "intersect"
            | "into"
            | "lateral"
            | "leading"
            | "limit"
            | "localtime"
            | "localtimestamp"
            | "not"
            | "null"
            | "offset"
            | "on"
            | "only"
            | "or"
            | "order"
            | "placing"
            | "primary"
            | "references"
            | "returning"
            | "select"
            | "session_user"
            | "set"
            | "some"
            | "symmetric"
            | "table"
            | "then"
            | "to"
            | "trailing"
            | "true"
            | "union"
            | "unique"
            | "update"
            | "user"
            | "using"
            | "variadic"
            | "when"
            | "where"
            | "window"
            | "with"
    )
}

fn placeholders(count: usize) -> String {
    (1..=count)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn identity_sequence_options_sql(options: &IdentitySequenceOptions) -> String {
    let cycle = if options.cycle { " CYCLE" } else { "" };

    format!(
        " ( INCREMENT {} START {} MINVALUE {} MAXVALUE {} CACHE {}{} )",
        options.increment,
        options.start,
        options.min_value,
        options.max_value,
        options.cache,
        cycle
    )
}

fn create_index_if_not_exists(definition: &str) -> String {
    if let Some(rest) = definition.strip_prefix("CREATE UNIQUE INDEX ") {
        return format!("CREATE UNIQUE INDEX IF NOT EXISTS {rest}");
    }

    if let Some(rest) = definition.strip_prefix("CREATE INDEX ") {
        return format!("CREATE INDEX IF NOT EXISTS {rest}");
    }

    definition.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        COLUMN_DEFINITION_SQL, ColumnDefinition, ConstraintDefinition, IdentitySequenceOptions,
        IndexDefinition, TableDefinition, build_create_script, delete_script, insert_script,
        select_script, update_script,
    };
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::table_browser::{ColumnTypeGroup, TableColumn};

    #[test]
    fn builds_dml_scripts_with_quoted_identifiers() {
        let object = table();
        let columns = vec![column("id"), column("path"), column("session_data")];

        assert_eq!(
            select_script(&object, &columns),
            "SELECT id, path, session_data\n    FROM analytics.page_views;"
        );
        assert_eq!(
            insert_script(&object, &columns),
            "INSERT INTO analytics.page_views(\n    id, path, session_data)\n    VALUES ($1, $2, $3);"
        );
        assert_eq!(
            update_script(&object, &columns),
            "UPDATE analytics.page_views\n    SET id = $1, path = $2, session_data = $3\n    WHERE <condition>;"
        );
        assert_eq!(
            delete_script(&object),
            "DELETE FROM analytics.page_views\n    WHERE <condition>;"
        );
    }

    #[test]
    fn builds_create_script_from_catalog_metadata() {
        let script = build_create_script(
            &table(),
            &TableDefinition {
                owner: "postgres".to_string(),
                tablespace: None,
            },
            &[
                ColumnDefinition {
                    name: "id".to_string(),
                    data_type: "bigint".to_string(),
                    collation: None,
                    is_not_null: true,
                    default_expression: None,
                    identity: "a".to_string(),
                    identity_sequence: Some(IdentitySequenceOptions {
                        increment: 1,
                        start: 1,
                        min_value: 1,
                        max_value: 9223372036854775807,
                        cache: 1,
                        cycle: false,
                    }),
                    generated: String::new(),
                },
                ColumnDefinition {
                    name: "viewed_at".to_string(),
                    data_type: "timestamp with time zone".to_string(),
                    collation: None,
                    is_not_null: true,
                    default_expression: Some("now()".to_string()),
                    identity: String::new(),
                    identity_sequence: None,
                    generated: String::new(),
                },
            ],
            &[ConstraintDefinition {
                name: "page_views_pkey".to_string(),
                definition: "PRIMARY KEY (id)".to_string(),
            }],
            &[IndexDefinition {
                schema: "analytics".to_string(),
                name: "page_views_session_data_gin_idx".to_string(),
                definition:
                    "CREATE INDEX page_views_session_data_gin_idx ON analytics.page_views USING gin (session_data)"
                        .to_string(),
            }],
        );

        assert!(script.contains("CREATE TABLE IF NOT EXISTS analytics.page_views"));
        assert!(script.contains(
            "id bigint NOT NULL GENERATED ALWAYS AS IDENTITY ( INCREMENT 1 START 1 MINVALUE 1 MAXVALUE 9223372036854775807 CACHE 1 )"
        ));
        assert!(script.contains("viewed_at timestamp with time zone NOT NULL DEFAULT now()"));
        assert!(script.contains("CONSTRAINT page_views_pkey PRIMARY KEY (id)"));
        assert!(!script.contains("TABLESPACE"));
        assert!(script.contains("ALTER TABLE IF EXISTS analytics.page_views"));
        assert!(script.contains("CREATE INDEX IF NOT EXISTS page_views_session_data_gin_idx"));
    }

    #[test]
    fn quotes_reserved_identifiers() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "order".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let columns = vec![column("select"), column("value")];

        assert_eq!(
            select_script(&object, &columns),
            "SELECT \"select\", value\n    FROM public.\"order\";"
        );
        assert_eq!(
            insert_script(&object, &columns),
            "INSERT INTO public.\"order\"(\n    \"select\", value)\n    VALUES ($1, $2);"
        );
    }

    #[test]
    fn includes_explicit_table_tablespace() {
        let script = build_create_script(
            &table(),
            &TableDefinition {
                owner: "postgres".to_string(),
                tablespace: Some("fast_space".to_string()),
            },
            &[ColumnDefinition {
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                collation: None,
                is_not_null: true,
                default_expression: None,
                identity: String::new(),
                identity_sequence: None,
                generated: String::new(),
            }],
            &[],
            &[],
        );

        assert!(script.contains(")\n\nTABLESPACE fast_space;"));
    }

    #[test]
    fn catalog_queries_cast_postgres_internal_char_fields() {
        assert!(COLUMN_DEFINITION_SQL.contains("a.attidentity::text AS identity"));
        assert!(COLUMN_DEFINITION_SQL.contains("a.attgenerated::text AS generated"));
    }

    fn table() -> DatabaseObject {
        DatabaseObject {
            schema: "analytics".to_string(),
            name: "page_views".to_string(),
            kind: DatabaseObjectKind::Table,
        }
    }

    fn column(name: &str) -> TableColumn {
        TableColumn {
            name: name.to_string(),
            display_type: "text".to_string(),
            type_name: "text".to_string(),
            enum_values: Vec::new(),
            type_group: ColumnTypeGroup::Text,
            is_array: false,
            is_range: false,
            is_nullable: false,
            is_primary_key: false,
            has_default: false,
            is_identity: false,
            is_generated: false,
            ordinal_position: 1,
        }
    }
}
