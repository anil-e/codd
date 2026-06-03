use crate::models::query_result::{
    MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT, QueryExecutionResult, QueryResult,
};
use crate::models::table_browser::TableCell;
use futures_util::{TryStream, TryStreamExt};
use sqlx::postgres::{PgQueryResult, PgRow, PgValueFormat, PgValueRef};
use sqlx::{Column, Either, PgPool, Row, TypeInfo, ValueRef, raw_sql};
use sqlx_core::type_checking::TypeChecking;
use std::fmt::Write;

type QueryStreamItem = Either<PgQueryResult, PgRow>;

pub async fn execute(
    pool: &PgPool,
    sql: &str,
    row_limit: usize,
) -> Result<QueryExecutionResult, sqlx::Error> {
    let row_limit = safe_row_limit(row_limit);
    let mut stream = raw_sql(sql).fetch_many(pool);

    collect_query_stream(&mut stream, sql, row_limit).await
}

pub async fn execute_read_only(
    pool: &PgPool,
    sql: &str,
    row_limit: usize,
) -> Result<QueryExecutionResult, sqlx::Error> {
    let row_limit = safe_row_limit(row_limit);
    validate_read_only_export_sql(sql)?;
    let mut connection = pool.acquire().await?;

    sqlx::query("BEGIN READ ONLY")
        .execute(&mut *connection)
        .await?;

    let result = {
        let mut stream = raw_sql(sql).fetch_many(&mut *connection);
        collect_query_stream_with_format(&mut stream, sql, row_limit, ValueFormat::Export).await
    };

    let rollback_result = sqlx::query("ROLLBACK").execute(&mut *connection).await;

    match (result, rollback_result) {
        (Ok(result), Ok(_)) => Ok(result),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

async fn collect_query_stream(
    stream: &mut (impl TryStream<Ok = QueryStreamItem, Error = sqlx::Error> + Unpin),
    sql: &str,
    row_limit: usize,
) -> Result<QueryExecutionResult, sqlx::Error> {
    collect_query_stream_with_format(stream, sql, row_limit, ValueFormat::Display).await
}

async fn collect_query_stream_with_format(
    stream: &mut (impl TryStream<Ok = QueryStreamItem, Error = sqlx::Error> + Unpin),
    sql: &str,
    row_limit: usize,
    value_format: ValueFormat,
) -> Result<QueryExecutionResult, sqlx::Error> {
    let mut current_result: Option<QueryResult> = None;
    let mut last_result = None;

    while let Some(item) = stream.try_next().await? {
        match item {
            Either::Left(result) => {
                if let Some(result) = current_result.take() {
                    last_result = Some(QueryExecutionResult::Rows(result));
                } else {
                    let rows = result.rows_affected();
                    last_result = Some(QueryExecutionResult::AffectedRows(rows));
                }
            }
            Either::Right(row) => {
                let row_limit_reached = {
                    let result =
                        current_result.get_or_insert_with(|| limited_query_result(row_limit));
                    if result.rows.len() >= row_limit {
                        result.row_limit_reached = true;
                        true
                    } else {
                        append_row_to_result(result, &row, value_format);
                        false
                    }
                };

                if row_limit_reached {
                    return Ok(QueryExecutionResult::Rows(
                        current_result.expect("limited query result to exist"),
                    ));
                }
            }
        }
    }

    if let Some(result) = current_result {
        Ok(QueryExecutionResult::Rows(result))
    } else if let Some(result) = last_result {
        if matches!(result, QueryExecutionResult::AffectedRows(0))
            && expects_rows_when_empty(last_sql_statement(sql))
        {
            return Ok(QueryExecutionResult::Rows(QueryResult::default()));
        }

        Ok(result)
    } else if expects_rows_when_empty(sql) {
        Ok(QueryExecutionResult::Rows(QueryResult::default()))
    } else {
        Ok(QueryExecutionResult::AffectedRows(0))
    }
}

fn limited_query_result(row_limit: usize) -> QueryResult {
    QueryResult {
        row_limit: Some(row_limit),
        ..QueryResult::default()
    }
}

pub(crate) fn safe_row_limit(row_limit: usize) -> usize {
    row_limit.clamp(MIN_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT)
}

fn last_sql_statement(sql: &str) -> &str {
    sql_statements(sql).last().copied().unwrap_or_default()
}

pub(crate) fn has_multiple_statements(sql: &str) -> bool {
    count_sql_statements(sql) > 1
}

fn count_sql_statements(sql: &str) -> usize {
    sql_statements(sql).len()
}

fn sql_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut statement_start = 0;
    let bytes = sql.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' if is_escape_string_quote(sql, index) => {
                index = skip_escape_quoted_string(bytes, index + 1);
            }
            b'\'' => index = skip_single_quoted_string(bytes, index + 1),
            b'"' => index = skip_double_quoted_identifier(bytes, index + 1),
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = skip_line_comment(bytes, index + 2);
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
            }
            b'$' if let Some(end) = skip_dollar_quoted_string(sql, index) => {
                index = end;
            }
            b';' => {
                let statement = sql[statement_start..index].trim();
                if is_effective_sql_statement(statement) {
                    statements.push(statement);
                }

                statement_start = index + 1;
                index += 1;
            }
            _ => index += 1,
        }
    }

    let statement = sql[statement_start..].trim();
    if is_effective_sql_statement(statement) {
        statements.push(statement);
    }

    statements
}

fn is_effective_sql_statement(statement: &str) -> bool {
    !strip_leading_sql_comments(statement).trim().is_empty()
}

fn validate_read_only_export_sql(sql: &str) -> Result<(), sqlx::Error> {
    if contains_transaction_control_statement(sql) {
        return Err(sqlx::Error::Protocol(
            "Transaction control statements cannot be exported.".to_string(),
        ));
    }

    Ok(())
}

fn contains_transaction_control_statement(sql: &str) -> bool {
    sql_statements(sql).into_iter().any(|statement| {
        let keywords = leading_sql_keywords(statement, 2);
        let first = keywords.first().map(String::as_str);
        let second = keywords.get(1).map(String::as_str);

        matches!(
            first,
            Some("begin")
                | Some("start")
                | Some("commit")
                | Some("rollback")
                | Some("end")
                | Some("abort")
                | Some("savepoint")
                | Some("release")
        ) || matches!((first, second), (Some("set"), Some("transaction")))
    })
}

fn leading_sql_keywords(statement: &str, limit: usize) -> Vec<String> {
    let mut keywords = Vec::with_capacity(limit);
    let statement = strip_leading_sql_comments(statement).trim_start();
    let bytes = statement.as_bytes();
    let mut index = 0;

    while index < bytes.len() && keywords.len() < limit {
        match bytes[index] {
            byte if is_identifier_start(byte) => {
                let start = index;
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| is_identifier_continue(*byte))
                {
                    index += 1;
                }
                keywords.push(statement[start..index].to_ascii_lowercase());
            }
            byte if byte.is_ascii_whitespace() => index += 1,
            _ => break,
        }
    }

    keywords
}

fn expects_rows_when_empty(sql: &str) -> bool {
    let normalized = strip_leading_sql_comments(sql)
        .trim_start()
        .to_ascii_lowercase();

    normalized.starts_with("select")
        || normalized.starts_with("with")
        || normalized.starts_with("show")
        || normalized.starts_with("explain")
        || normalized.starts_with("values")
        || contains_keyword_outside_literals(&normalized, "returning")
}

pub(crate) fn changes_schema(sql: &str) -> bool {
    let normalized = sql.to_ascii_lowercase();

    [
        "alter", "comment", "create", "drop", "grant", "reindex", "revoke",
    ]
    .into_iter()
    .any(|keyword| contains_keyword_outside_literals(&normalized, keyword))
}

fn strip_leading_sql_comments(mut sql: &str) -> &str {
    loop {
        sql = sql.trim_start();

        if let Some(rest) = sql.strip_prefix("--") {
            sql = rest.split_once('\n').map_or("", |(_, rest)| rest);
            continue;
        }

        if let Some(rest) = sql.strip_prefix("/*") {
            let Some((_, after_comment)) = rest.split_once("*/") else {
                return "";
            };
            sql = after_comment;
            continue;
        }

        return sql;
    }
}

fn contains_keyword_outside_literals(sql: &str, keyword: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'\'' if is_escape_string_quote(sql, index) => {
                index = skip_escape_quoted_string(bytes, index + 1);
            }
            b'\'' => index = skip_single_quoted_string(bytes, index + 1),
            b'"' => index = skip_double_quoted_identifier(bytes, index + 1),
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = skip_line_comment(bytes, index + 2);
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
            }
            b'$' if let Some(end) = skip_dollar_quoted_string(sql, index) => {
                index = end;
            }
            byte if is_identifier_start(byte) => {
                let start = index;
                index += 1;
                while bytes
                    .get(index)
                    .is_some_and(|byte| is_identifier_continue(*byte))
                {
                    index += 1;
                }

                if &sql[start..index] == keyword {
                    return true;
                }
            }
            _ => index += 1,
        }
    }

    false
}

fn skip_single_quoted_string(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        if bytes[index] == b'\'' {
            index += 1;
            if bytes.get(index) == Some(&b'\'') {
                index += 1;
            } else {
                return index;
            }
        } else {
            index += 1;
        }
    }

    index
}

fn skip_escape_quoted_string(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'\'' => {
                index += 1;
                if bytes.get(index) == Some(&b'\'') {
                    index += 1;
                } else {
                    return index;
                }
            }
            _ => index += 1,
        }
    }

    index
}

fn skip_double_quoted_identifier(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() {
        if bytes[index] == b'"' {
            index += 1;
            if bytes.get(index) == Some(&b'"') {
                index += 1;
            } else {
                return index;
            }
        } else {
            index += 1;
        }
    }

    index
}

fn skip_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }

    index
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> usize {
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }

        index += 1;
    }

    bytes.len()
}

fn skip_dollar_quoted_string(sql: &str, start: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let tag_end = bytes
        .get(start + 1..)?
        .iter()
        .position(|byte| *byte == b'$')?
        + start
        + 1;

    if !bytes[start + 1..tag_end]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return None;
    }

    let delimiter = &sql[start..=tag_end];
    let content_start = tag_end + 1;
    let relative_end = sql[content_start..].find(delimiter)?;

    Some(content_start + relative_end + delimiter.len())
}

fn is_escape_string_quote(sql: &str, quote_index: usize) -> bool {
    let bytes = sql.as_bytes();

    if quote_index == 0 || !matches!(bytes.get(quote_index - 1), Some(b'e' | b'E')) {
        return false;
    }

    quote_index < 2 || !is_identifier_continue(bytes[quote_index - 2])
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ValueFormat {
    Display,
    Export,
}

#[cfg(test)]
fn rows_to_result(rows: &[PgRow]) -> QueryResult {
    let mut result = QueryResult::default();

    for row in rows {
        append_row_to_result(&mut result, row, ValueFormat::Display);
    }

    result
}

fn append_row_to_result(result: &mut QueryResult, row: &PgRow, value_format: ValueFormat) {
    if result.columns.is_empty() {
        result.columns = row
            .columns()
            .iter()
            .map(|column| column.name().to_string())
            .collect();
    }

    result.rows.push(
        row.columns()
            .iter()
            .enumerate()
            .map(|(index, column)| {
                value_to_string_with_format(row, index, column.type_info().name(), value_format)
            })
            .collect(),
    );
}

pub(crate) fn value_to_cell(row: &PgRow, index: usize, type_name: &str) -> TableCell {
    value_to_cell_with_format(row, index, type_name, ValueFormat::Display)
}

pub(crate) fn value_to_export_cell(row: &PgRow, index: usize, type_name: &str) -> TableCell {
    value_to_cell_with_format(row, index, type_name, ValueFormat::Export)
}

fn value_to_string_with_format(
    row: &PgRow,
    index: usize,
    type_name: &str,
    value_format: ValueFormat,
) -> String {
    value_to_cell_with_format(row, index, type_name, value_format).value
}

fn value_to_cell_with_format(
    row: &PgRow,
    index: usize,
    type_name: &str,
    value_format: ValueFormat,
) -> TableCell {
    if row
        .try_get_raw(index)
        .map(|value| value.is_null())
        .unwrap_or(false)
    {
        return TableCell::null();
    }

    let value = match type_name {
        "BOOL" => decode::<bool>(row, index),
        "INT2" => decode::<i16>(row, index),
        "INT4" => decode::<i32>(row, index),
        "INT8" => decode::<i64>(row, index),
        "FLOAT4" => decode::<f32>(row, index),
        "FLOAT8" => decode::<f64>(row, index),
        "CHAR" => decode::<String>(row, index),
        "\"CHAR\"" => decode_pg_char(row, index),
        "JSON" | "JSONB" => decode_json(row, index, value_format),
        "DATE" => decode::<chrono::NaiveDate>(row, index),
        "TIME" => decode::<chrono::NaiveTime>(row, index),
        "TIMESTAMP" => decode::<chrono::NaiveDateTime>(row, index),
        "TIMESTAMPTZ" => decode::<chrono::DateTime<chrono::Utc>>(row, index),
        _ => decode::<String>(row, index),
    };

    TableCell::new(value)
}

fn decode<T>(row: &PgRow, index: usize) -> String
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + ToString,
{
    row.try_get::<T, _>(index).map_or_else(
        |_| fallback_value_to_string(row, index),
        |value| value.to_string(),
    )
}

fn decode_pg_char(row: &PgRow, index: usize) -> String {
    row.try_get::<i8, _>(index).map_or_else(
        |_| {
            row.try_get_raw(index).map_or_else(
                |_| "<unavailable>".to_string(),
                |value| raw_value_to_string(&value),
            )
        },
        |value| {
            let byte = value as u8;
            char::from(byte).to_string()
        },
    )
}

fn decode_json(row: &PgRow, index: usize, value_format: ValueFormat) -> String {
    row.try_get::<serde_json::Value, _>(index).map_or_else(
        |_| fallback_value_to_string(row, index),
        |value| json_to_string(&value, value_format),
    )
}

fn json_to_string(value: &serde_json::Value, value_format: ValueFormat) -> String {
    let result = match value_format {
        ValueFormat::Display => serde_json::to_string_pretty(value),
        ValueFormat::Export => serde_json::to_string(value),
    };

    result.unwrap_or_else(|_| value.to_string())
}

fn fallback_value_to_string(row: &PgRow, index: usize) -> String {
    row.try_get_raw(index).map_or_else(
        |_| "<unavailable>".to_string(),
        |value| {
            let owned = <PgValueRef<'_> as ValueRef>::to_owned(&value);
            let debug = format!("{:?}", sqlx::Postgres::fmt_value_debug(&owned));

            if debug.starts_with("(unknown SQL type ")
                || debug.starts_with("(error decoding SQL type ")
            {
                raw_value_to_string(&value)
            } else {
                debug
            }
        },
    )
}

fn raw_value_to_string(value: &PgValueRef<'_>) -> String {
    match value.format() {
        PgValueFormat::Text => value
            .as_str()
            .map_or_else(|_| raw_bytes_to_hex(value), str::to_owned),
        PgValueFormat::Binary => raw_bytes_to_hex(value),
    }
}

fn raw_bytes_to_hex(value: &PgValueRef<'_>) -> String {
    value.as_bytes().map_or_else(
        |_| "<unavailable>".to_string(),
        |bytes| {
            let mut output = String::with_capacity(bytes.len() * 2 + 2);
            output.push_str("\\x");

            for byte in bytes {
                let _ = write!(&mut output, "{byte:02x}");
            }

            output
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ValueFormat, changes_schema, contains_keyword_outside_literals, expects_rows_when_empty,
        has_multiple_statements, json_to_string, last_sql_statement, rows_to_result,
        safe_row_limit, validate_read_only_export_sql,
    };
    use crate::models::query_result::{MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT};

    #[test]
    fn detects_row_returning_statements() {
        assert!(expects_rows_when_empty(
            "insert into users(name) values ('a') returning id"
        ));
        assert!(expects_rows_when_empty(
            "update users set name = 'a' returning id"
        ));
        assert!(expects_rows_when_empty("delete from users returning id"));
        assert!(expects_rows_when_empty(
            "with updated as (update users set name = 'a' returning id) select * from updated"
        ));
    }

    #[test]
    fn ignores_leading_comments_when_detecting_row_statements() {
        assert!(expects_rows_when_empty("-- comment\nselect 1"));
        assert!(expects_rows_when_empty("/* comment */ values (1)"));
        assert!(expects_rows_when_empty(
            "/* first */ -- second\n insert into users(name) values ('a') returning id"
        ));
    }

    #[test]
    fn treats_plain_dml_as_affected_rows() {
        assert!(!expects_rows_when_empty(
            "insert into users(name) values ('a')"
        ));
        assert!(!expects_rows_when_empty("update users set name = 'a'"));
        assert!(!expects_rows_when_empty("delete from users"));
    }

    #[test]
    fn detects_common_empty_row_statements() {
        assert!(expects_rows_when_empty("select * from users where false"));
        assert!(expects_rows_when_empty(
            "with users as (select 1) select * from users where false"
        ));
        assert!(expects_rows_when_empty("show search_path"));
        assert!(expects_rows_when_empty("explain select 1"));
        assert!(expects_rows_when_empty("values (1), (2)"));
    }

    #[test]
    fn returning_detection_requires_a_standalone_keyword() {
        assert!(!expects_rows_when_empty(
            "insert into returning_log(message) values ('done')"
        ));
        assert!(!expects_rows_when_empty(
            "insert into users(name) values ('returning')"
        ));
        assert!(!expects_rows_when_empty(
            "insert into users(returning_value) values ('a')"
        ));
    }

    #[test]
    fn returning_detection_ignores_literals_identifiers_and_comments() {
        assert!(!contains_keyword_outside_literals(
            "'returning'",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            "\"returning\"",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            "-- returning\ninsert into users(name) values ('a')",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            "/* returning */ insert into users(name) values ('a')",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            "$$ returning $$",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            "$body$ returning $body$",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            r"E'returning \\'",
            "returning"
        ));
        assert!(!contains_keyword_outside_literals(
            "returning_value",
            "returning"
        ));
        assert!(contains_keyword_outside_literals(
            "returning id",
            "returning"
        ));
    }

    #[test]
    fn handles_empty_or_comment_only_sql() {
        assert!(!expects_rows_when_empty(""));
        assert!(!expects_rows_when_empty("   "));
        assert!(!expects_rows_when_empty("-- comment without newline"));
        assert!(!expects_rows_when_empty("/* unterminated comment"));
        assert!(!expects_rows_when_empty("/* complete */"));
    }

    #[test]
    fn ignores_comment_only_statements() {
        assert!(!has_multiple_statements("-- comment;\nselect 1"));
        assert_eq!(last_sql_statement("select 1; -- comment"), "select 1");
        assert_eq!(last_sql_statement("/* comment */; select 2"), "select 2");
    }

    #[test]
    fn detects_schema_changes_outside_literals_and_comments() {
        assert!(changes_schema("create table users(id bigint)"));
        assert!(changes_schema(
            "select 1; alter table users add column name text"
        ));
        assert!(changes_schema("drop table users"));
        assert!(changes_schema("comment on table users is 'public users'"));

        assert!(!changes_schema("-- drop table users\nselect 1"));
        assert!(!changes_schema("select 'create table users(id bigint)'"));
        assert!(!changes_schema("select * from create_log"));
        assert!(!changes_schema("insert into users(name) values ('a')"));
    }

    #[test]
    fn finds_last_sql_statement() {
        assert_eq!(last_sql_statement("select 1"), "select 1");
        assert_eq!(last_sql_statement("select 1;"), "select 1");
        assert_eq!(last_sql_statement("select 1; select 2"), "select 2");
        assert_eq!(
            last_sql_statement("select ';'; update users set name = 'a'"),
            "update users set name = 'a'"
        );
        assert_eq!(
            last_sql_statement("select $$;$$; update users set name = 'a'"),
            "update users set name = 'a'"
        );
        assert_eq!(
            last_sql_statement("select $body$;$body$; select 2"),
            "select 2"
        );
        assert_eq!(last_sql_statement(r"select E'a\';'; select 2"), "select 2");
        assert_eq!(last_sql_statement("select 1; -- tail comment"), "select 1");
    }

    #[test]
    fn detects_multiple_statements_outside_literals_and_comments() {
        assert!(!has_multiple_statements("select 1"));
        assert!(!has_multiple_statements("select 1;"));
        assert!(!has_multiple_statements("select ';'"));
        assert!(!has_multiple_statements("-- select 1;\nselect 1"));
        assert!(!has_multiple_statements("select $$;$$"));
        assert!(has_multiple_statements("select 1; select 2"));
    }

    #[test]
    fn empty_results_are_supported() {
        let result = rows_to_result(&[]);

        assert!(result.columns.is_empty());
        assert!(result.rows.is_empty());
    }

    #[test]
    fn row_limit_is_clamped_to_safe_bounds() {
        assert_eq!(safe_row_limit(0), MIN_QUERY_RESULT_ROW_LIMIT);
        assert_eq!(safe_row_limit(1_000), 1_000);
        assert_eq!(
            safe_row_limit(MAX_QUERY_RESULT_ROW_LIMIT + 1),
            MAX_QUERY_RESULT_ROW_LIMIT
        );
    }

    #[test]
    fn display_json_is_pretty_printed() {
        let value = serde_json::json!({
            "name": "Ada",
            "tags": ["sql", "rust"],
        });

        assert!(json_to_string(&value, ValueFormat::Display).contains('\n'));
    }

    #[test]
    fn exported_json_is_compact() {
        let value = serde_json::json!({
            "name": "Ada",
            "tags": ["sql", "rust"],
        });

        assert_eq!(
            json_to_string(&value, ValueFormat::Export),
            r#"{"name":"Ada","tags":["sql","rust"]}"#
        );
    }

    #[test]
    fn read_only_export_rejects_transaction_control_statements() {
        assert!(
            validate_read_only_export_sql("select 1; commit; insert into users values (1)")
                .is_err()
        );
        assert!(validate_read_only_export_sql("rollback; select 1").is_err());
        assert!(validate_read_only_export_sql("set transaction read write; select 1").is_err());
    }

    #[test]
    fn read_only_export_allows_transaction_words_in_selects() {
        assert!(validate_read_only_export_sql("select commit from audit_log").is_ok());
        assert!(validate_read_only_export_sql("select 'commit; rollback'").is_ok());
        assert!(validate_read_only_export_sql("-- commit\nselect 1").is_ok());
    }
}
