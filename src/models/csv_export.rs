use crate::models::query_result::QueryResult;
use crate::models::table_browser::TablePage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvDelimiter {
    Comma,
    Semicolon,
    Tab,
}

impl CsvDelimiter {
    pub fn character(self) -> char {
        match self {
            Self::Comma => ',',
            Self::Semicolon => ';',
            Self::Tab => '\t',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvExportOptions {
    pub delimiter: CsvDelimiter,
    pub row_limit: usize,
}

impl Default for CsvExportOptions {
    fn default() -> Self {
        Self {
            delimiter: CsvDelimiter::Comma,
            row_limit: 1_000,
        }
    }
}

pub fn query_result(result: &QueryResult, options: CsvExportOptions) -> String {
    let mut rows = Vec::with_capacity(result.rows.len() + 1);
    rows.push(format_row(
        result.columns.iter().map(String::as_str),
        options.delimiter,
    ));

    rows.extend(
        result
            .rows
            .iter()
            .map(|row| format_row(row.iter().map(String::as_str), options.delimiter)),
    );

    rows.join("\r\n")
}

pub fn table_page(page: &TablePage, options: CsvExportOptions) -> String {
    let mut rows = Vec::with_capacity(page.rows.len() + 1);
    rows.push(format_row(
        page.columns.iter().map(|column| column.name.as_str()),
        options.delimiter,
    ));

    rows.extend(page.rows.iter().map(|row| {
        format_row(
            row.iter().map(|cell| cell.value.as_str()),
            options.delimiter,
        )
    }));

    rows.join("\r\n")
}

fn format_row<'a>(fields: impl IntoIterator<Item = &'a str>, delimiter: CsvDelimiter) -> String {
    let delimiter_char = delimiter.character();

    fields
        .into_iter()
        .map(|field| format_field(field, delimiter_char))
        .collect::<Vec<_>>()
        .join(&delimiter_char.to_string())
}

fn format_field(value: &str, delimiter: char) -> String {
    if !value.contains(['"', '\n', '\r']) && !value.contains(delimiter) {
        return value.to_string();
    }

    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{CsvDelimiter, CsvExportOptions, query_result, table_page};
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::query_result::QueryResult;
    use crate::models::table_browser::{ColumnTypeGroup, TableCell, TableColumn, TablePage};

    #[test]
    fn formats_query_result_with_comma_delimiter_and_header() {
        let result = query_result_fixture();

        assert_eq!(
            query_result(&result, options(CsvDelimiter::Comma)),
            "id,name\r\n1,Ada\r\n2,Grace"
        );
    }

    #[test]
    fn formats_query_result_with_semicolon_delimiter() {
        let result = query_result_fixture();

        assert_eq!(
            query_result(&result, options(CsvDelimiter::Semicolon)),
            "id;name\r\n1;Ada\r\n2;Grace"
        );
    }

    #[test]
    fn formats_query_result_with_tab_delimiter() {
        let result = query_result_fixture();

        assert_eq!(
            query_result(&result, options(CsvDelimiter::Tab)),
            "id\tname\r\n1\tAda\r\n2\tGrace"
        );
    }

    #[test]
    fn escapes_quotes_newlines_and_delimiter() {
        let result = QueryResult {
            columns: vec!["id".to_string(), "notes".to_string()],
            rows: vec![vec!["1".to_string(), "line 1\nline, \"2\"".to_string()]],
            row_limit: None,
            row_limit_reached: false,
        };

        assert_eq!(
            query_result(&result, options(CsvDelimiter::Comma)),
            "id,notes\r\n1,\"line 1\nline, \"\"2\"\"\""
        );
    }

    #[test]
    fn writes_header_for_empty_query_result() {
        let result = QueryResult {
            columns: vec!["id".to_string(), "name".to_string()],
            rows: Vec::new(),
            row_limit: None,
            row_limit_reached: false,
        };

        assert_eq!(
            query_result(&result, options(CsvDelimiter::Comma)),
            "id,name"
        );
    }

    #[test]
    fn formats_table_page_with_header() {
        let page = TablePage {
            object: DatabaseObject {
                schema: "public".to_string(),
                name: "users".to_string(),
                kind: DatabaseObjectKind::Table,
            },
            columns: vec![column("id"), column("name")],
            rows: vec![vec![
                TableCell::new("1".to_string()),
                TableCell::new("Ada".to_string()),
            ]],
            offset: 0,
            page_size: 1,
            has_next_page: false,
        };

        assert_eq!(
            table_page(&page, options(CsvDelimiter::Comma)),
            "id,name\r\n1,Ada"
        );
    }

    fn query_result_fixture() -> QueryResult {
        QueryResult {
            columns: vec!["id".to_string(), "name".to_string()],
            rows: vec![
                vec!["1".to_string(), "Ada".to_string()],
                vec!["2".to_string(), "Grace".to_string()],
            ],
            row_limit: None,
            row_limit_reached: false,
        }
    }

    fn options(delimiter: CsvDelimiter) -> CsvExportOptions {
        CsvExportOptions {
            delimiter,
            row_limit: 1_000,
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
