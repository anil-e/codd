use crate::models::database_object::quote_identifier;

const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "FULL",
    "ON",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "ALTER",
    "DROP",
    "TABLE",
    "VIEW",
    "INDEX",
    "TRIGGER",
    "FUNCTION",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "DISTINCT",
    "TRUE",
    "FALSE",
    "AS",
    "ASC",
    "DESC",
    "RETURNING",
];

const FUNCTIONS: &[SqlFunction] = &[
    SqlFunction::new("ABS"),
    SqlFunction::new("ARRAY_AGG"),
    SqlFunction::new("ARRAY_LENGTH"),
    SqlFunction::new("AVG"),
    SqlFunction::new("CEIL"),
    SqlFunction::new("COALESCE"),
    SqlFunction::new("CONCAT"),
    SqlFunction::new("COUNT"),
    SqlFunction::new("DATE_PART"),
    SqlFunction::new("DATE_TRUNC"),
    SqlFunction::new("DENSE_RANK"),
    SqlFunction::new("EXTRACT"),
    SqlFunction::new("FLOOR"),
    SqlFunction::new("GREATEST"),
    SqlFunction::new("JSONB_AGG"),
    SqlFunction::new("JSONB_ARRAY_LENGTH"),
    SqlFunction::new("JSONB_BUILD_ARRAY"),
    SqlFunction::new("JSONB_BUILD_OBJECT"),
    SqlFunction::new("JSONB_EACH"),
    SqlFunction::new("JSONB_EACH_TEXT"),
    SqlFunction::new("JSONB_EXTRACT_PATH"),
    SqlFunction::new("JSONB_EXTRACT_PATH_TEXT"),
    SqlFunction::new("JSONB_OBJECT_KEYS"),
    SqlFunction::new("JSONB_PRETTY"),
    SqlFunction::new("JSONB_SET"),
    SqlFunction::new("JSONB_STRIP_NULLS"),
    SqlFunction::new("JSONB_TYPEOF"),
    SqlFunction::new("LEAST"),
    SqlFunction::new("LENGTH"),
    SqlFunction::new("LOWER"),
    SqlFunction::new("MAX"),
    SqlFunction::new("MIN"),
    SqlFunction::new("NOW"),
    SqlFunction::new("NULLIF"),
    SqlFunction::new("RANK"),
    SqlFunction::new("REGEXP_REPLACE"),
    SqlFunction::new("REPLACE"),
    SqlFunction::new("ROUND"),
    SqlFunction::new("ROW_NUMBER"),
    SqlFunction::new("SPLIT_PART"),
    SqlFunction::new("STRING_AGG"),
    SqlFunction::new("SUBSTRING"),
    SqlFunction::new("SUM"),
    SqlFunction::new("TO_CHAR"),
    SqlFunction::new("TO_DATE"),
    SqlFunction::new("TO_JSONB"),
    SqlFunction::new("TRIM"),
    SqlFunction::new("UPPER"),
];

const MAX_COMPLETION_SUGGESTIONS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SqlFunction {
    name: &'static str,
}

impl SqlFunction {
    const fn new(name: &'static str) -> Self {
        Self { name }
    }

    fn insert_text(self) -> String {
        format!("{}()", self.name)
    }

    fn detail(self) -> String {
        format!(
            "Kind: {}\nSignature: {}()",
            CompletionItemKind::Function.label(),
            self.name
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionCatalog {
    pub schemas: Vec<CompletionSchema>,
    pub objects: Vec<CompletionObject>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSchema {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionObject {
    pub schema: String,
    pub name: String,
    pub kind: CompletionItemKind,
    pub columns: Vec<CompletionColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionColumn {
    pub name: String,
    pub data_type: String,
    pub ordinal_position: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionItemKind {
    Keyword,
    Function,
    Schema,
    Table,
    View,
    MaterializedView,
    Column,
}

impl CompletionItemKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Function => "function",
            Self::Schema => "schema",
            Self::Table => "table",
            Self::View => "view",
            Self::MaterializedView => "materialized view",
            Self::Column => "column",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSuggestion {
    pub label: String,
    pub insert_text: String,
    pub summary: String,
    pub detail: String,
    pub kind: CompletionItemKind,
    pub cursor_backward: usize,
}

impl CompletionCatalog {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn suggestions(&self, sql_before_cursor: &str) -> Vec<CompletionSuggestion> {
        self.suggestions_with_context(sql_before_cursor, sql_before_cursor)
    }

    pub fn suggestions_with_context(
        &self,
        sql_before_cursor: &str,
        sql_context: &str,
    ) -> Vec<CompletionSuggestion> {
        let context = CompletionContext::from_sql(sql_before_cursor, sql_context);

        match context.qualifier.as_deref() {
            Some(qualifier) => {
                self.qualified_suggestions(qualifier, &context.prefix, &context.table_refs)
            }
            None => self.unqualified_suggestions(&context.prefix, &context.table_refs),
        }
    }

    fn unqualified_suggestions(
        &self,
        prefix: &str,
        table_refs: &[QueryTableRef],
    ) -> Vec<CompletionSuggestion> {
        let mut suggestions = Vec::new();

        for table_ref in table_refs {
            if let Some(object) = self.resolve_table_ref(table_ref) {
                for column in &object.columns {
                    if matches_prefix(&column.name, prefix) {
                        suggestions.push(CompletionSuggestion {
                            label: column.name.clone(),
                            insert_text: quoted_if_needed(&column.name),
                            summary: CompletionItemKind::Column.label().to_string(),
                            detail: column_detail(object, column),
                            kind: CompletionItemKind::Column,
                            cursor_backward: 0,
                        });
                    }
                }
            }
        }

        for schema in &self.schemas {
            if matches_prefix(&schema.name, prefix) {
                suggestions.push(CompletionSuggestion {
                    label: schema.name.clone(),
                    insert_text: quoted_if_needed(&schema.name),
                    summary: CompletionItemKind::Schema.label().to_string(),
                    detail: format!("Kind: {}", CompletionItemKind::Schema.label()),
                    kind: CompletionItemKind::Schema,
                    cursor_backward: 0,
                });
            }
        }

        for object in &self.objects {
            if matches_prefix(&object.name, prefix) {
                suggestions.push(CompletionSuggestion {
                    label: object.name.clone(),
                    insert_text: quoted_if_needed(&object.name),
                    summary: object.kind.label().to_string(),
                    detail: object_detail(object),
                    kind: object.kind,
                    cursor_backward: 0,
                });
            }
        }

        for function in FUNCTIONS {
            if matches_prefix(function.name, prefix) {
                suggestions.push(CompletionSuggestion {
                    label: function.name.to_string(),
                    insert_text: function.insert_text(),
                    summary: CompletionItemKind::Function.label().to_string(),
                    detail: function.detail(),
                    kind: CompletionItemKind::Function,
                    cursor_backward: 1,
                });
            }
        }

        for keyword in KEYWORDS {
            if matches_prefix(keyword, prefix) {
                suggestions.push(CompletionSuggestion {
                    label: (*keyword).to_string(),
                    insert_text: (*keyword).to_string(),
                    summary: CompletionItemKind::Keyword.label().to_string(),
                    detail: CompletionItemKind::Keyword.label().to_string(),
                    kind: CompletionItemKind::Keyword,
                    cursor_backward: 0,
                });
            }
        }

        sort_suggestions(&mut suggestions, prefix);
        suggestions.dedup_by(|left, right| {
            left.label == right.label && left.kind == right.kind && left.detail == right.detail
        });
        suggestions.truncate(MAX_COMPLETION_SUGGESTIONS);

        suggestions
    }

    fn qualified_suggestions(
        &self,
        qualifier: &str,
        prefix: &str,
        table_refs: &[QueryTableRef],
    ) -> Vec<CompletionSuggestion> {
        if let Some((schema, object_name)) = qualifier.rsplit_once('.')
            && let Some(object) = self.resolve_qualified_object(schema, object_name)
        {
            return self.column_suggestions(object, prefix);
        }

        let qualifier = unquote_identifier(qualifier);

        if let Some(object) = table_refs
            .iter()
            .find(|table_ref| table_ref.alias.eq_ignore_ascii_case(&qualifier))
            .and_then(|table_ref| self.resolve_table_ref(table_ref))
        {
            return self.column_suggestions(object, prefix);
        }

        let mut schema_matches = self
            .objects
            .iter()
            .filter(|object| object.schema.eq_ignore_ascii_case(&qualifier))
            .filter(|object| matches_prefix(&object.name, prefix))
            .map(|object| CompletionSuggestion {
                label: object.name.clone(),
                insert_text: quoted_if_needed(&object.name),
                summary: object.kind.label().to_string(),
                detail: object_detail(object),
                kind: object.kind,
                cursor_backward: 0,
            })
            .collect::<Vec<_>>();

        if !schema_matches.is_empty() {
            sort_suggestions(&mut schema_matches, prefix);
            schema_matches.truncate(MAX_COMPLETION_SUGGESTIONS);

            return schema_matches;
        }

        let matching_objects = self
            .objects
            .iter()
            .filter(|object| object.name.eq_ignore_ascii_case(&qualifier))
            .collect::<Vec<_>>();
        if matching_objects.len() != 1 {
            return Vec::new();
        }

        self.column_suggestions(matching_objects[0], prefix)
    }

    fn resolve_table_ref(&self, table_ref: &QueryTableRef) -> Option<&CompletionObject> {
        self.objects.iter().find(|object| {
            object.name.eq_ignore_ascii_case(&table_ref.name)
                && table_ref
                    .schema
                    .as_ref()
                    .is_none_or(|schema| object.schema.eq_ignore_ascii_case(schema))
        })
    }

    fn resolve_qualified_object(
        &self,
        schema: &str,
        object_name: &str,
    ) -> Option<&CompletionObject> {
        let schema = unquote_identifier(schema);
        let object_name = unquote_identifier(object_name);

        self.objects.iter().find(|object| {
            object.schema.eq_ignore_ascii_case(&schema)
                && object.name.eq_ignore_ascii_case(&object_name)
        })
    }

    fn column_suggestions(
        &self,
        object: &CompletionObject,
        prefix: &str,
    ) -> Vec<CompletionSuggestion> {
        let mut columns = object
            .columns
            .iter()
            .filter(|column| matches_prefix(&column.name, prefix))
            .collect::<Vec<_>>();

        columns.sort_by(|left, right| {
            suggestion_score(&left.name, prefix)
                .cmp(&suggestion_score(&right.name, prefix))
                .then_with(|| left.ordinal_position.cmp(&right.ordinal_position))
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
        });
        columns.truncate(MAX_COMPLETION_SUGGESTIONS);

        columns
            .into_iter()
            .map(|column| CompletionSuggestion {
                label: column.name.clone(),
                insert_text: quoted_if_needed(&column.name),
                summary: CompletionItemKind::Column.label().to_string(),
                detail: column_detail(object, column),
                kind: CompletionItemKind::Column,
                cursor_backward: 0,
            })
            .collect()
    }
}

fn column_detail(object: &CompletionObject, column: &CompletionColumn) -> String {
    format!(
        "Kind: {}\nTable: {}.{}\nType: {}",
        CompletionItemKind::Column.label(),
        object.schema,
        object.name,
        column.data_type
    )
}

fn object_detail(object: &CompletionObject) -> String {
    format!(
        "Kind: {}\nSchema: {}\nName: {}",
        object.kind.label(),
        object.schema,
        object.name
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletionContext {
    qualifier: Option<String>,
    prefix: String,
    table_refs: Vec<QueryTableRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryTableRef {
    alias: String,
    schema: Option<String>,
    name: String,
}

impl CompletionContext {
    fn from_sql(sql_before_cursor: &str, sql_context: &str) -> Self {
        let token = trailing_completion_token(sql_before_cursor);
        let table_refs = query_table_refs(sql_context);

        if let Some((qualifier, prefix)) = token.rsplit_once('.') {
            return Self {
                qualifier: Some(qualifier.to_string()),
                prefix: prefix.to_string(),
                table_refs,
            };
        }

        Self {
            qualifier: None,
            prefix: token.to_string(),
            table_refs,
        }
    }
}

fn sort_suggestions(suggestions: &mut [CompletionSuggestion], prefix: &str) {
    suggestions.sort_by(|left, right| {
        suggestion_score(&left.label, prefix)
            .cmp(&suggestion_score(&right.label, prefix))
            .then_with(|| kind_rank(left.kind).cmp(&kind_rank(right.kind)))
            .then_with(|| {
                left.label
                    .to_ascii_lowercase()
                    .cmp(&right.label.to_ascii_lowercase())
            })
    });
}

fn suggestion_score(value: &str, prefix: &str) -> u8 {
    if prefix.is_empty() || value.eq_ignore_ascii_case(prefix) {
        0
    } else if value
        .to_ascii_lowercase()
        .starts_with(&prefix.to_ascii_lowercase())
    {
        1
    } else {
        2
    }
}

fn kind_rank(kind: CompletionItemKind) -> u8 {
    match kind {
        CompletionItemKind::Schema => 0,
        CompletionItemKind::Table => 1,
        CompletionItemKind::View => 2,
        CompletionItemKind::MaterializedView => 3,
        CompletionItemKind::Column => 4,
        CompletionItemKind::Function => 5,
        CompletionItemKind::Keyword => 6,
    }
}

fn matches_prefix(value: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || value
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
}

fn trailing_completion_token(sql: &str) -> &str {
    let end = sql.len();
    let start = sql
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!is_completion_token_char(character)).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);

    &sql[start..end]
}

fn is_completion_token_char(character: char) -> bool {
    character == '.' || character == '_' || character == '"' || character.is_alphanumeric()
}

fn quoted_if_needed(identifier: &str) -> String {
    if is_simple_identifier(identifier) {
        identifier.to_string()
    } else {
        quote_identifier(identifier)
    }
}

fn is_simple_identifier(identifier: &str) -> bool {
    let mut chars = identifier.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first == '_' || first.is_ascii_lowercase())
        && chars.all(|character| {
            character == '_' || character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && !is_reserved_identifier(identifier)
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

fn unquote_identifier(identifier: &str) -> String {
    identifier
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .map(|value| value.replace("\"\"", "\""))
        .unwrap_or_else(|| identifier.to_string())
}

fn query_table_refs(sql: &str) -> Vec<QueryTableRef> {
    let tokens = sql_tokens(sql);
    let mut refs = Vec::new();
    let mut index = 0;

    while let Some(token) = tokens.get(index) {
        if !matches_keyword(token, "FROM") && !matches_keyword(token, "JOIN") {
            index += 1;
            continue;
        }

        let Some((table_ref, next_index)) = parse_table_ref(&tokens, index + 1) else {
            index += 1;
            continue;
        };

        refs.push(table_ref);
        index = parse_comma_separated_table_refs(&tokens, next_index, &mut refs);
    }

    refs
}

fn parse_comma_separated_table_refs(
    tokens: &[String],
    mut index: usize,
    refs: &mut Vec<QueryTableRef>,
) -> usize {
    while tokens.get(index).is_some_and(|token| token == ",") {
        let Some((table_ref, next_index)) = parse_table_ref(tokens, index + 1) else {
            break;
        };

        refs.push(table_ref);
        index = next_index;
    }

    index
}

fn parse_table_ref(tokens: &[String], start: usize) -> Option<(QueryTableRef, usize)> {
    let first = tokens.get(start)?;
    if is_clause_boundary(first) {
        return None;
    }

    let mut schema = None;
    let mut name = first.clone();
    let mut index = start + 1;

    if tokens.get(index).is_some_and(|token| token == ".")
        && let Some(second) = tokens.get(index + 1)
    {
        schema = Some(name);
        name = second.clone();
        index += 2;
    }

    if tokens
        .get(index)
        .is_some_and(|token| matches_keyword(token, "AS"))
    {
        index += 1;
    }

    let alias = tokens
        .get(index)
        .filter(|token| !is_clause_boundary(token) && token.as_str() != ",")
        .cloned()
        .unwrap_or_else(|| name.clone());

    Some((
        QueryTableRef {
            alias,
            schema,
            name,
        },
        index + 1,
    ))
}

fn sql_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = sql.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '\'' => skip_single_quoted_string(&mut chars),
            '"' => tokens.push(read_quoted_identifier(&mut chars)),
            '-' if chars.peek() == Some(&'-') => skip_line_comment(&mut chars),
            '/' if chars.peek() == Some(&'*') => skip_block_comment(&mut chars),
            '.' | ',' => tokens.push(character.to_string()),
            character if character == '_' || character.is_alphanumeric() => {
                tokens.push(read_identifier(character, &mut chars));
            }
            _ => {}
        }
    }

    tokens
}

fn read_identifier(first: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut identifier = first.to_string();

    while let Some(character) = chars
        .next_if(|character| *character == '_' || *character == '$' || character.is_alphanumeric())
    {
        identifier.push(character);
    }

    identifier
}

fn read_quoted_identifier(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut identifier = String::new();

    while let Some(character) = chars.next() {
        if character == '"' {
            if chars.next_if_eq(&'"').is_some() {
                identifier.push('"');
                continue;
            }

            break;
        }

        identifier.push(character);
    }

    identifier
}

fn skip_single_quoted_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(character) = chars.next() {
        if character == '\'' && chars.next_if_eq(&'\'').is_none() {
            break;
        }
    }
}

fn skip_line_comment(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for character in chars.by_ref() {
        if character == '\n' {
            break;
        }
    }
}

fn skip_block_comment(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    let _ = chars.next_if_eq(&'*');
    while let Some(character) = chars.next() {
        if character == '*' && chars.next_if_eq(&'/').is_some() {
            break;
        }
    }
}

fn matches_keyword(token: &str, keyword: &str) -> bool {
    token.eq_ignore_ascii_case(keyword)
}

fn is_clause_boundary(token: &str) -> bool {
    [
        "SELECT", "FROM", "WHERE", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "ON",
        "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION",
    ]
    .iter()
    .any(|keyword| matches_keyword(token, keyword))
}

#[cfg(test)]
mod tests {
    use super::{
        CompletionCatalog, CompletionColumn, CompletionItemKind, CompletionObject,
        CompletionSchema, CompletionSuggestion,
    };

    #[test]
    fn suggests_unqualified_schema_objects_and_keywords() {
        let catalog = test_catalog();
        let labels = catalog
            .suggestions("select * from us")
            .into_iter()
            .map(|suggestion| suggestion.label)
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["users"]);
    }

    #[test]
    fn suggests_common_postgresql_functions() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions("select co");

        assert_eq!(
            suggestion_labels(&suggestions),
            vec!["COALESCE", "CONCAT", "COUNT"]
        );
        assert!(
            suggestions
                .iter()
                .all(|suggestion| suggestion.kind == CompletionItemKind::Function)
        );
        assert_eq!(suggestions[0].insert_text, "COALESCE()");
        assert_eq!(suggestions[0].cursor_backward, 1);
    }

    #[test]
    fn suggests_jsonb_postgresql_functions() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions("select jsonb_pr");

        assert_eq!(suggestion_labels(&suggestions), vec!["JSONB_PRETTY"]);
        assert_eq!(suggestions[0].insert_text, "JSONB_PRETTY()");
        assert_eq!(suggestions[0].kind, CompletionItemKind::Function);
    }

    #[test]
    fn suggests_objects_after_schema_qualifier() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions("select * from analytics.");

        assert_eq!(
            suggestion_labels(&suggestions),
            vec!["page_views", "session summary"]
        );
    }

    #[test]
    fn suggests_columns_after_unique_table_qualifier() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions("select users.");

        assert_eq!(suggestion_labels(&suggestions), vec!["id", "name", "email"]);
    }

    #[test]
    fn suggests_columns_after_schema_and_table_qualifier() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions("select analytics.page_views.");

        assert_eq!(suggestion_labels(&suggestions), vec!["path"]);
    }

    #[test]
    fn suggests_columns_after_alias_qualifier() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions("select * from users u where u.");

        assert_eq!(suggestion_labels(&suggestions), vec!["id", "name", "email"]);
    }

    #[test]
    fn suggests_columns_for_alias_declared_after_cursor() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions_with_context(
            "select c.",
            "select c.\nfrom users c\nleft join analytics.page_views p on p.path = c.email",
        );

        assert_eq!(suggestion_labels(&suggestions), vec!["id", "name", "email"]);
    }

    #[test]
    fn suggests_unqualified_columns_from_tables_declared_after_cursor() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions_with_context(
            "select na",
            "select na\nfrom users c\nleft join analytics.page_views p on p.path = c.email",
        );

        assert_eq!(suggestion_labels(&suggestions), vec!["name"]);
    }

    #[test]
    fn prioritizes_columns_from_from_and_join_context() {
        let catalog = test_catalog();
        let suggestions =
            catalog.suggestions("select * from users u join analytics.page_views p on e");

        assert_eq!(suggestions[0].label, "email");
        assert_eq!(suggestions[0].kind, CompletionItemKind::Column);
    }

    #[test]
    fn suggests_columns_from_comma_separated_table_refs() {
        let catalog = test_catalog();
        let suggestions =
            catalog.suggestions("select * from users u, analytics.page_views p where p.");

        assert_eq!(suggestion_labels(&suggestions), vec!["path"]);
    }

    #[test]
    fn ignores_from_and_join_inside_strings_and_comments() {
        let catalog = test_catalog();
        let suggestions = catalog.suggestions(
            "select 'from users u' -- join analytics.page_views p\nfrom analytics.page_views p where p.",
        );

        assert_eq!(suggestion_labels(&suggestions), vec!["path"]);
    }

    #[test]
    fn quotes_insert_text_for_identifiers_that_need_it() {
        let catalog = test_catalog();
        let suggestion = catalog
            .suggestions("select * from analytics.session ")
            .into_iter()
            .find(|suggestion| suggestion.label == "session summary")
            .unwrap();

        assert_eq!(suggestion.insert_text, "\"session summary\"");
    }

    #[test]
    fn quotes_insert_text_for_reserved_identifiers() {
        let catalog = CompletionCatalog {
            schemas: vec![CompletionSchema {
                name: "public".to_string(),
            }],
            objects: vec![CompletionObject {
                schema: "public".to_string(),
                name: "user".to_string(),
                kind: CompletionItemKind::Table,
                columns: vec![column("order", "integer", 1)],
            }],
        };

        let object = catalog
            .suggestions("select * from us")
            .into_iter()
            .find(|suggestion| suggestion.label == "user")
            .unwrap();
        let column = catalog
            .suggestions("select public.user.")
            .into_iter()
            .find(|suggestion| suggestion.label == "order")
            .unwrap();

        assert_eq!(object.insert_text, "\"user\"");
        assert_eq!(column.insert_text, "\"order\"");
    }

    #[test]
    fn limits_large_completion_lists() {
        let catalog = CompletionCatalog {
            schemas: vec![CompletionSchema {
                name: "public".to_string(),
            }],
            objects: (0..250)
                .map(|index| CompletionObject {
                    schema: "public".to_string(),
                    name: format!("table_{index}"),
                    kind: CompletionItemKind::Table,
                    columns: Vec::new(),
                })
                .collect(),
        };

        assert_eq!(catalog.suggestions("select * from ").len(), 200);
    }

    fn suggestion_labels(suggestions: &[CompletionSuggestion]) -> Vec<String> {
        suggestions
            .iter()
            .map(|suggestion| suggestion.label.clone())
            .collect()
    }

    fn test_catalog() -> CompletionCatalog {
        CompletionCatalog {
            schemas: vec![
                CompletionSchema {
                    name: "analytics".to_string(),
                },
                CompletionSchema {
                    name: "public".to_string(),
                },
            ],
            objects: vec![
                CompletionObject {
                    schema: "public".to_string(),
                    name: "users".to_string(),
                    kind: CompletionItemKind::Table,
                    columns: vec![
                        column("id", "bigint", 1),
                        column("name", "text", 2),
                        column("email", "text", 3),
                    ],
                },
                CompletionObject {
                    schema: "analytics".to_string(),
                    name: "page_views".to_string(),
                    kind: CompletionItemKind::Table,
                    columns: vec![column("path", "text", 1)],
                },
                CompletionObject {
                    schema: "analytics".to_string(),
                    name: "session summary".to_string(),
                    kind: CompletionItemKind::View,
                    columns: vec![column("session_id", "uuid", 1)],
                },
            ],
        }
    }

    fn column(name: &str, data_type: &str, ordinal_position: i32) -> CompletionColumn {
        CompletionColumn {
            name: name.to_string(),
            data_type: data_type.to_string(),
            ordinal_position,
        }
    }
}
