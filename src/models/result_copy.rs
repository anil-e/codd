use crate::models::query_result::QueryResult;
use crate::models::table_browser::TablePage;

pub fn cell(result: &QueryResult, row_index: usize, column_index: usize) -> Option<String> {
    result.rows.get(row_index)?.get(column_index).cloned()
}

pub fn row(result: &QueryResult, row_index: usize) -> Option<String> {
    result.rows.get(row_index).map(|row| format_tsv_row(row))
}

pub fn column(result: &QueryResult, column_index: usize) -> Option<String> {
    result.columns.get(column_index)?;

    Some(
        result
            .rows
            .iter()
            .filter_map(|row| row.get(column_index))
            .map(|value| format_tsv_fields([value.as_str()]))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn table(result: &QueryResult) -> String {
    let mut rows = Vec::with_capacity(result.rows.len() + 1);
    rows.push(format_tsv_row(&result.columns));

    rows.extend(result.rows.iter().map(|row| format_tsv_row(row)));

    rows.join("\n")
}

pub fn page_cell(page: &TablePage, row_index: usize, column_index: usize) -> Option<String> {
    page.rows
        .get(row_index)?
        .get(column_index)
        .map(|cell| cell.value.clone())
}

pub fn page_row(page: &TablePage, row_index: usize) -> Option<String> {
    page.rows
        .get(row_index)
        .map(|row| format_tsv_fields(row.iter().map(|cell| cell.value.as_str())))
}

pub fn page_column(page: &TablePage, column_index: usize) -> Option<String> {
    page.columns.get(column_index)?;

    Some(
        page.rows
            .iter()
            .filter_map(|row| row.get(column_index))
            .map(|cell| format_tsv_fields([cell.value.as_str()]))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn page(page: &TablePage) -> String {
    let mut rows = Vec::with_capacity(page.rows.len() + 1);
    rows.push(format_tsv_fields(
        page.columns.iter().map(|column| column.name.as_str()),
    ));

    rows.extend(
        page.rows
            .iter()
            .map(|row| format_tsv_fields(row.iter().map(|cell| cell.value.as_str()))),
    );

    rows.join("\n")
}

fn format_tsv_row(row: &[String]) -> String {
    format_tsv_fields(row.iter().map(String::as_str))
}

fn format_tsv_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
    fields
        .into_iter()
        .map(format_tsv_field)
        .collect::<Vec<_>>()
        .join("\t")
}

fn format_tsv_field(value: &str) -> String {
    if !value.contains(['\t', '\n', '\r', '"']) {
        return value.to_string();
    }

    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{cell, column, row, table};
    use crate::models::query_result::QueryResult;

    fn result() -> QueryResult {
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

    #[test]
    fn copies_cell() {
        assert_eq!(cell(&result(), 1, 1), Some("Grace".to_string()));
    }

    #[test]
    fn copies_row_as_tsv() {
        assert_eq!(row(&result(), 0), Some("1\tAda".to_string()));
    }

    #[test]
    fn copies_column_as_lines() {
        assert_eq!(column(&result(), 1), Some("Ada\nGrace".to_string()));
    }

    #[test]
    fn copies_table_with_header_as_tsv() {
        assert_eq!(table(&result()), "id\tname\n1\tAda\n2\tGrace");
    }

    #[test]
    fn escapes_structural_tsv_characters() {
        let result = QueryResult {
            columns: vec!["id".to_string(), "notes".to_string()],
            rows: vec![vec!["1".to_string(), "line 1\nline\t\"2\"".to_string()]],
            row_limit: None,
            row_limit_reached: false,
        };

        assert_eq!(
            row(&result, 0),
            Some("1\t\"line 1\nline\t\"\"2\"\"\"".to_string())
        );
        assert_eq!(table(&result), "id\tnotes\n1\t\"line 1\nline\t\"\"2\"\"\"");
    }

    #[test]
    fn returns_none_for_missing_targets() {
        assert_eq!(cell(&result(), 9, 0), None);
        assert_eq!(row(&result(), 9), None);
        assert_eq!(column(&result(), 9), None);
    }
}
