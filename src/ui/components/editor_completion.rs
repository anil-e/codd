use std::cell::RefCell;

use glib::subclass::prelude::*;
use relm4::gtk;
use relm4::gtk::{gdk, gio, glib};
use sourceview5::prelude::*;
use sourceview5::subclass::prelude::*;

use crate::models::completion::{CompletionCatalog, CompletionItemKind, CompletionSuggestion};

glib::wrapper! {
    pub struct SqlCompletionProposal(ObjectSubclass<proposal::SqlCompletionProposal>)
        @implements sourceview5::CompletionProposal;
}

impl SqlCompletionProposal {
    fn new(suggestion: CompletionSuggestion) -> Self {
        let proposal: Self = glib::Object::new();
        proposal.imp().suggestion.replace(Some(suggestion));
        proposal
    }

    fn suggestion(&self) -> CompletionSuggestion {
        self.imp()
            .suggestion
            .borrow()
            .as_ref()
            .expect("completion proposal to contain suggestion")
            .clone()
    }
}

mod proposal {
    use super::*;

    #[derive(Default)]
    pub struct SqlCompletionProposal {
        pub(super) suggestion: RefCell<Option<CompletionSuggestion>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SqlCompletionProposal {
        const NAME: &'static str = "CoddSqlCompletionProposal";

        type Type = super::SqlCompletionProposal;
        type Interfaces = (sourceview5::CompletionProposal,);
    }

    impl ObjectImpl for SqlCompletionProposal {}
    impl CompletionProposalImpl for SqlCompletionProposal {}
}

glib::wrapper! {
    pub struct SqlCompletionProvider(ObjectSubclass<provider::SqlCompletionProvider>)
        @implements sourceview5::CompletionProvider;
}

impl SqlCompletionProvider {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_catalog(&self, catalog: CompletionCatalog) {
        self.imp().catalog.replace(catalog);
    }
}

impl Default for SqlCompletionProvider {
    fn default() -> Self {
        Self::new()
    }
}

mod provider {
    use super::*;

    #[derive(Default)]
    pub struct SqlCompletionProvider {
        pub(super) catalog: RefCell<CompletionCatalog>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SqlCompletionProvider {
        const NAME: &'static str = "CoddSqlCompletionProvider";

        type Type = super::SqlCompletionProvider;
        type Interfaces = (sourceview5::CompletionProvider,);
    }

    impl ObjectImpl for SqlCompletionProvider {}

    impl CompletionProviderImpl for SqlCompletionProvider {
        fn title(&self) -> Option<glib::GString> {
            Some("Codd".into())
        }

        fn priority(&self, _context: &sourceview5::CompletionContext) -> i32 {
            100
        }

        fn is_trigger(&self, _iter: &gtk::TextIter, character: char) -> bool {
            is_completion_trigger(character)
        }

        fn key_activates(
            &self,
            _context: &sourceview5::CompletionContext,
            _proposal: &sourceview5::CompletionProposal,
            keyval: gdk::Key,
            state: gdk::ModifierType,
        ) -> bool {
            keyval == gdk::Key::Return && state.is_empty()
        }

        fn populate(
            &self,
            context: &sourceview5::CompletionContext,
        ) -> Result<gio::ListModel, glib::Error> {
            Ok(self.proposals(context))
        }

        fn refilter(&self, context: &sourceview5::CompletionContext, model: &gio::ListModel) {
            let Some(model) = model.downcast_ref::<gio::ListStore>() else {
                return;
            };

            self.update_model(context, model);
        }

        fn display(
            &self,
            _context: &sourceview5::CompletionContext,
            proposal: &sourceview5::CompletionProposal,
            cell: &sourceview5::CompletionCell,
        ) {
            let Some(proposal) = proposal.downcast_ref::<SqlCompletionProposal>() else {
                return;
            };
            let suggestion = proposal.suggestion();

            match cell.column() {
                sourceview5::CompletionColumn::Icon => {
                    cell.set_icon_name(icon_name(suggestion.kind));
                }
                sourceview5::CompletionColumn::TypedText => {
                    cell.set_text(Some(&suggestion.label));
                }
                sourceview5::CompletionColumn::Comment => {
                    cell.set_text(Some(&suggestion.summary));
                }
                sourceview5::CompletionColumn::Details => {
                    cell.set_text(Some(&suggestion.detail));
                }
                _ => {}
            }
        }

        fn activate(
            &self,
            context: &sourceview5::CompletionContext,
            proposal: &sourceview5::CompletionProposal,
        ) {
            let Some(proposal) = proposal.downcast_ref::<SqlCompletionProposal>() else {
                return;
            };
            let Some(buffer) = context.buffer() else {
                return;
            };
            let Some((mut begin, mut end)) = context.bounds() else {
                return;
            };

            let suggestion = proposal.suggestion();
            buffer.delete(&mut begin, &mut end);
            buffer.insert(&mut begin, &suggestion.insert_text);
            if suggestion.cursor_backward > 0 {
                begin.backward_chars(suggestion.cursor_backward as i32);
                buffer.place_cursor(&begin);
            }
        }
    }
}

impl provider::SqlCompletionProvider {
    fn proposals(&self, context: &sourceview5::CompletionContext) -> gio::ListModel {
        let model = gio::ListStore::new::<SqlCompletionProposal>();
        self.update_model(context, &model);

        model.upcast()
    }

    fn update_model(&self, context: &sourceview5::CompletionContext, model: &gio::ListStore) {
        model.remove_all();

        let Some(sql_context) = sql_completion_context(context) else {
            return;
        };

        for suggestion in self
            .catalog
            .borrow()
            .suggestions_with_context(&sql_context.before_cursor, &sql_context.full_sql)
        {
            model.append(&SqlCompletionProposal::new(suggestion));
        }
    }
}

struct SqlCompletionContext {
    before_cursor: String,
    full_sql: String,
}

fn sql_completion_context(
    context: &sourceview5::CompletionContext,
) -> Option<SqlCompletionContext> {
    let buffer = context.buffer()?;
    let cursor = context
        .bounds()
        .map(|(_, end)| end)
        .unwrap_or_else(|| buffer.iter_at_offset(buffer.cursor_position()));
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    let before_cursor = buffer.text(&start, &cursor, true).to_string();
    let full_sql = buffer.text(&start, &end, true).to_string();
    let statement_sql = current_statement_sql(&full_sql, cursor.offset());

    Some(SqlCompletionContext {
        before_cursor,
        full_sql: statement_sql,
    })
}

fn current_statement_sql(sql: &str, cursor_offset: i32) -> String {
    let cursor_index = byte_index_for_char_offset(sql, cursor_offset.max(0) as usize);
    let start = statement_start(sql, cursor_index);
    let end = statement_end(sql, cursor_index);

    sql[start..end].to_string()
}

fn statement_start(sql: &str, cursor_index: usize) -> usize {
    let mut start = 0;
    let mut scanner = SqlScanner::default();

    for (index, character) in sql[..cursor_index].char_indices() {
        if scanner.advance(sql, index, character) && character == ';' {
            start = index + character.len_utf8();
        }
    }

    start
}

fn statement_end(sql: &str, cursor_index: usize) -> usize {
    let mut scanner = SqlScanner::default();

    for (relative_index, character) in sql[cursor_index..].char_indices() {
        let index = cursor_index + relative_index;
        if scanner.advance(sql, index, character) && character == ';' {
            return index;
        }
    }

    sql.len()
}

fn byte_index_for_char_offset(text: &str, offset: usize) -> usize {
    text.char_indices()
        .nth(offset)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

#[derive(Debug, Default)]
struct SqlScanner {
    state: SqlScannerState,
}

#[derive(Debug, Default)]
enum SqlScannerState {
    #[default]
    Normal,
    SingleQuotedString,
    DoubleQuotedIdentifier,
    LineComment,
    BlockComment,
}

impl SqlScanner {
    fn advance(&mut self, sql: &str, index: usize, character: char) -> bool {
        match self.state {
            SqlScannerState::Normal => match character {
                '\'' => {
                    self.state = SqlScannerState::SingleQuotedString;
                    false
                }
                '"' => {
                    self.state = SqlScannerState::DoubleQuotedIdentifier;
                    false
                }
                '-' if sql[index + 1..].starts_with('-') => {
                    self.state = SqlScannerState::LineComment;
                    false
                }
                '/' if sql[index + 1..].starts_with('*') => {
                    self.state = SqlScannerState::BlockComment;
                    false
                }
                _ => true,
            },
            SqlScannerState::SingleQuotedString => {
                if character == '\''
                    && !sql[index + 1..].starts_with('\'')
                    && !sql[..index].ends_with('\'')
                {
                    self.state = SqlScannerState::Normal;
                }

                false
            }
            SqlScannerState::DoubleQuotedIdentifier => {
                if character == '"'
                    && !sql[index + 1..].starts_with('"')
                    && !sql[..index].ends_with('"')
                {
                    self.state = SqlScannerState::Normal;
                }

                false
            }
            SqlScannerState::LineComment => {
                if character == '\n' {
                    self.state = SqlScannerState::Normal;
                }

                false
            }
            SqlScannerState::BlockComment => {
                if character == '/' && index > 0 && sql[..index].ends_with('*') {
                    self.state = SqlScannerState::Normal;
                }

                false
            }
        }
    }
}

fn is_completion_trigger(character: char) -> bool {
    character == '.' || character == '_' || character.is_alphanumeric()
}

fn icon_name(kind: CompletionItemKind) -> &'static str {
    match kind {
        CompletionItemKind::Keyword => "code-symbolic",
        CompletionItemKind::Function => "lang-function-symbolic",
        CompletionItemKind::Schema => "folder-symbolic",
        CompletionItemKind::Table => "table-symbolic",
        CompletionItemKind::View => "view-list-symbolic",
        CompletionItemKind::MaterializedView => "view-list-symbolic",
        CompletionItemKind::Column => "columns-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::current_statement_sql;

    #[test]
    fn current_statement_uses_statement_around_cursor() {
        let sql = "select * from users;\nselect c.\nfrom customers c;";
        let cursor = sql.find("c.").unwrap() + 2;

        assert_eq!(
            current_statement_sql(sql, cursor as i32).trim(),
            "select c.\nfrom customers c"
        );
    }

    #[test]
    fn current_statement_ignores_semicolons_inside_strings_and_comments() {
        let sql = "select ';', ''';';\nselect c.\n-- ;\nfrom customers c;";
        let cursor = sql.find("c.").unwrap() + 2;

        assert_eq!(
            current_statement_sql(sql, cursor as i32).trim(),
            "select c.\n-- ;\nfrom customers c"
        );
    }
}
