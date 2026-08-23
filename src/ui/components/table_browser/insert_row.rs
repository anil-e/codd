use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_browser::{ColumnTypeGroup, TableCell, TableColumn, TableInsertValue};
use crate::ui::components::table_browser::{
    InsertRowKind, InsertRowRequest, InsertRowResult, TableBrowser, TableBrowserCommandOutput,
    TableBrowserMsg, TableBrowserOutput,
};

const COLUMN_LABEL_WIDTH: i32 = 210;
const VALUE_ENTRY_MIN_WIDTH: i32 = 320;
const MODE_DROPDOWN_WIDTH: i32 = 120;

#[derive(Clone)]
struct InsertColumnInput {
    column_index: usize,
    value: InsertValueInput,
    mode: gtk::DropDown,
    modes: Vec<InsertMode>,
}

#[derive(Clone)]
enum InsertValueInput {
    Text(gtk::Entry),
    Choice {
        dropdown: gtk::DropDown,
        values: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InsertMode {
    Value,
    Null,
    Default,
}

impl TableBrowser {
    pub(super) fn open_insert_row_dialog(&self, root: &gtk::Box, sender: &ComponentSender<Self>) {
        let Some(page) = self.page.as_ref() else {
            return;
        };

        if page.object.kind != DatabaseObjectKind::Table {
            return;
        }

        show_insert_row_dialog(
            root.root().and_downcast::<gtk::Window>().as_ref(),
            &page.columns,
            None,
            None,
            &page.object,
            InsertRowKind::Insert,
            sender,
        );
    }

    pub(super) fn open_duplicate_row_dialog(
        &self,
        root: &gtk::Box,
        sender: &ComponentSender<Self>,
    ) {
        let Some(page) = self.page.as_ref() else {
            return;
        };

        if page.object.kind != DatabaseObjectKind::Table {
            return;
        }

        let Some((_, row)) = self.selected_row() else {
            return;
        };

        let Some(values) = duplicate_values(&page.columns, &row) else {
            self.show_warning(
                root,
                &gettext("Duplicating row failed"),
                &gettext("This row contains values that cannot be duplicated safely."),
            );

            return;
        };

        show_insert_row_dialog(
            root.root().and_downcast::<gtk::Window>().as_ref(),
            &page.columns,
            Some(&values),
            None,
            &page.object,
            InsertRowKind::Duplicate,
            sender,
        );
    }

    pub(super) fn reopen_insert_row_dialog(
        &self,
        root: &gtk::Box,
        request: InsertRowRequest,
        error: &str,
        sender: &ComponentSender<Self>,
    ) {
        if !self.insert_request_is_current(&request) {
            let heading = match request.kind {
                InsertRowKind::Insert => gettext("Inserting row failed"),
                InsertRowKind::Duplicate => gettext("Duplicating row failed"),
            };

            self.show_warning(
                root,
                &heading,
                &gettext("The submitted row is no longer valid."),
            );

            return;
        }

        show_insert_row_dialog(
            root.root().and_downcast::<gtk::Window>().as_ref(),
            &request.columns,
            Some(&request.values),
            Some(error),
            &request.object,
            request.kind,
            sender,
        );
    }

    pub(super) fn insert_request_is_current(&self, request: &InsertRowRequest) -> bool {
        insert_request_matches(self.object.as_ref(), &self.available_columns, request)
    }

    pub(super) fn insert_row(&mut self, request: InsertRowRequest, sender: &ComponentSender<Self>) {
        let Some(pool) = self.pool.clone() else {
            return;
        };

        self.is_loading = true;
        self.context_busy.set(true);
        let _ = sender.output(TableBrowserOutput::BusyChanged(true));

        let id = self.allocate_request_id();
        self.active_insert_request_id = Some(id);
        self.active_delete_request_id = None;

        sender.oneshot_command(async move {
            if let Err(error) = db::browser::insert_table_row(
                &pool,
                &request.object,
                &request.columns,
                &request.values,
            )
            .await
            {
                return TableBrowserCommandOutput::RowInserted {
                    id,
                    result: InsertRowResult::InsertFailed {
                        error: error.to_string(),
                        request,
                    },
                };
            }

            TableBrowserCommandOutput::RowInserted {
                id,
                result: InsertRowResult::Inserted(request.kind),
            }
        });
    }
}

fn show_insert_row_dialog(
    parent: Option<&gtk::Window>,
    columns: &[TableColumn],
    initial_values: Option<&[TableInsertValue]>,
    error: Option<&str>,
    object: &DatabaseObject,
    kind: InsertRowKind,
    sender: &ComponentSender<TableBrowser>,
) {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.add_css_class("insert-row-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    let mut inputs = Vec::new();

    for (index, column) in columns.iter().enumerate() {
        if column.is_insertable() {
            let input = insertable_column_row(
                index,
                column,
                initial_values.and_then(|values| values.get(index)),
            );
            list.append(&input.row);
            inputs.push(input.input);
        } else {
            list.append(&readonly_column_row(column));
        }
    }

    let scroller = gtk::ScrolledWindow::builder()
        .min_content_width(820)
        .min_content_height(300)
        .max_content_height(620)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .child(&list)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    if let Some(error) = error {
        let icon = gtk::Image::builder()
            .icon_name("dialog-error-symbolic")
            .valign(gtk::Align::Start)
            .margin_top(2)
            .build();
        icon.add_css_class("error");

        let message = gtk::Label::builder()
            .label(error)
            .hexpand(true)
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .build();
        message.add_css_class("error");

        let error_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        error_box.append(&icon);
        error_box.append(&message);
        content.append(&error_box);
    }

    content.append(&scroller);

    let heading = match (kind, error) {
        (InsertRowKind::Insert, Some(_)) => {
            gettext("Inserting row into {name} failed").replace("{name}", &object.qualified_name())
        }
        (InsertRowKind::Duplicate, Some(_)) => gettext("Duplicating row into {name} failed")
            .replace("{name}", &object.qualified_name()),
        (InsertRowKind::Insert, None) => gettext("Insert Row"),
        (InsertRowKind::Duplicate, None) => gettext("Duplicate Row"),
    };
    let body = error.map(|_| String::new()).unwrap_or_else(|| match kind {
        InsertRowKind::Insert => gettext(
            "Enter values for the new row. Columns can also use DEFAULT or NULL when available.",
        ),
        InsertRowKind::Duplicate => {
            gettext("Review the copied values before inserting the duplicate row.")
        }
    });
    let confirm_label = match kind {
        InsertRowKind::Insert => gettext("Insert"),
        InsertRowKind::Duplicate => gettext("Duplicate"),
    };

    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .extra_child(&content)
        .build();

    dialog.add_responses(&[("cancel", &gettext("Cancel")), ("insert", &confirm_label)]);
    dialog.set_default_response(Some("insert"));
    dialog.set_response_appearance("insert", adw::ResponseAppearance::Suggested);

    let inputs = std::rc::Rc::new(inputs);
    let columns = std::rc::Rc::new(columns.to_vec());
    let object = object.clone();

    sync_insert_response(&dialog, &columns, &inputs);
    for input in inputs.iter() {
        input.value.connect_changed({
            let dialog = dialog.clone();
            let columns = columns.clone();
            let inputs = inputs.clone();

            move || sync_insert_response(&dialog, &columns, &inputs)
        });

        input.mode.connect_selected_notify({
            let dialog = dialog.clone();
            let columns = columns.clone();
            let inputs = inputs.clone();
            let input = input.clone();

            move |_| {
                input.value.set_sensitive(input.mode() == InsertMode::Value);
                sync_insert_response(&dialog, &columns, &inputs);
            }
        });
    }

    let sender = sender.clone();
    dialog.choose(parent, None::<&gtk::gio::Cancellable>, move |response| {
        if response != "insert" {
            return;
        }

        let values = insert_values(&columns, &inputs);
        sender.input(TableBrowserMsg::InsertRowConfirmed(InsertRowRequest {
            kind,
            object,
            columns: columns.as_ref().clone(),
            values,
        }));
    });
}

struct InsertColumnRow {
    row: gtk::ListBoxRow,
    input: InsertColumnInput,
}

fn insertable_column_row(
    index: usize,
    column: &TableColumn,
    initial_value: Option<&TableInsertValue>,
) -> InsertColumnRow {
    let row = gtk::ListBoxRow::new();
    let grid = row_grid();
    row.set_child(Some(&grid));

    grid.attach(&column_label(column, column_subtitle(column)), 0, 0, 1, 1);

    let value = value_input(column, initial_value);
    grid.attach(&value.widget(), 1, 0, 1, 1);

    let (mode, modes) = mode_dropdown(column, initial_value);
    mode.set_sensitive(modes.len() > 1);
    grid.attach(&mode, 2, 0, 1, 1);

    let input = InsertColumnInput {
        column_index: index,
        value,
        mode,
        modes,
    };
    input.value.set_sensitive(input.mode() == InsertMode::Value);

    InsertColumnRow { row, input }
}

fn readonly_column_row(column: &TableColumn) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_selectable(false);

    let grid = row_grid();
    row.set_child(Some(&grid));

    grid.attach(&column_label(column, readonly_reason(column)), 0, 0, 1, 1);

    let entry = gtk::Entry::builder()
        .width_request(VALUE_ENTRY_MIN_WIDTH)
        .hexpand(true)
        .sensitive(false)
        .placeholder_text(readonly_reason(column))
        .build();
    grid.attach(&entry, 1, 0, 1, 1);

    let (mode, _) = readonly_mode_dropdown();
    grid.attach(&mode, 2, 0, 1, 1);

    row
}

fn mode_dropdown(
    column: &TableColumn,
    initial_value: Option<&TableInsertValue>,
) -> (gtk::DropDown, Vec<InsertMode>) {
    let mut labels = vec![gettext("Value")];
    let mut modes = vec![InsertMode::Value];
    if column.is_nullable {
        labels.push(gettext("NULL"));
        modes.push(InsertMode::Null);
    }
    if column.has_default {
        labels.push(gettext("Default"));
        modes.push(InsertMode::Default);
    }

    let default_mode = if column.has_default {
        modes
            .iter()
            .position(|mode| *mode == InsertMode::Default)
            .unwrap_or(0)
    } else if column.is_nullable {
        modes
            .iter()
            .position(|mode| *mode == InsertMode::Null)
            .unwrap_or(0)
    } else {
        0
    };
    let selected = initial_value
        .map(insert_mode_for_value)
        .and_then(|mode| modes.iter().position(|candidate| *candidate == mode))
        .unwrap_or(default_mode);

    let borrowed = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let dropdown = gtk::DropDown::builder()
        .model(&gtk::StringList::new(&borrowed))
        .selected(selected as u32)
        .width_request(MODE_DROPDOWN_WIDTH)
        .build();
    dropdown.add_css_class("compact");

    (dropdown, modes)
}

fn readonly_mode_dropdown() -> (gtk::DropDown, Vec<InsertMode>) {
    let label = gettext("Skipped");
    let labels = [label.as_str()];
    let dropdown = gtk::DropDown::builder()
        .model(&gtk::StringList::new(&labels))
        .sensitive(false)
        .width_request(MODE_DROPDOWN_WIDTH)
        .build();
    dropdown.add_css_class("compact");

    (dropdown, vec![InsertMode::Default])
}

impl InsertColumnInput {
    fn mode(&self) -> InsertMode {
        self.modes
            .get(self.mode.selected() as usize)
            .copied()
            .unwrap_or(InsertMode::Value)
    }
}

impl InsertValueInput {
    fn text(column: &TableColumn, initial_value: Option<&TableInsertValue>) -> Self {
        let entry = gtk::Entry::builder()
            .width_request(VALUE_ENTRY_MIN_WIDTH)
            .hexpand(true)
            .placeholder_text(value_placeholder(column))
            .build();

        if let Some(value) = insert_value_text(initial_value) {
            entry.set_text(value);
        }

        Self::Text(entry)
    }

    fn boolean(initial_value: Option<&TableInsertValue>) -> Self {
        let values = vec!["false".to_string(), "true".to_string()];
        let borrowed = values.iter().map(String::as_str).collect::<Vec<_>>();
        let selected = boolean_selection(initial_value);
        let dropdown = gtk::DropDown::builder()
            .model(&gtk::StringList::new(&borrowed))
            .selected(selected)
            .width_request(VALUE_ENTRY_MIN_WIDTH)
            .hexpand(true)
            .build();
        dropdown.add_css_class("compact");

        Self::Choice { dropdown, values }
    }

    fn widget(&self) -> gtk::Widget {
        match self {
            Self::Text(entry) => entry.clone().upcast(),
            Self::Choice { dropdown, .. } => dropdown.clone().upcast(),
        }
    }

    fn connect_changed(&self, callback: impl Fn() + Clone + 'static) {
        match self {
            Self::Text(entry) => {
                entry.connect_changed(move |_| callback());
            }
            Self::Choice { dropdown, .. } => {
                dropdown.connect_selected_notify(move |_| callback());
            }
        }
    }

    fn set_sensitive(&self, sensitive: bool) {
        match self {
            Self::Text(entry) => entry.set_sensitive(sensitive),
            Self::Choice { dropdown, .. } => dropdown.set_sensitive(sensitive),
        }
    }

    fn value(&self) -> String {
        match self {
            Self::Text(entry) => entry.text().to_string(),
            Self::Choice { dropdown, values } => values
                .get(dropdown.selected() as usize)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

fn value_input(column: &TableColumn, initial_value: Option<&TableInsertValue>) -> InsertValueInput {
    if column.type_group == ColumnTypeGroup::Boolean {
        return InsertValueInput::boolean(initial_value);
    }

    InsertValueInput::text(column, initial_value)
}

fn insert_mode_for_value(value: &TableInsertValue) -> InsertMode {
    match value {
        TableInsertValue::Default => InsertMode::Default,
        TableInsertValue::Null => InsertMode::Null,
        TableInsertValue::Value(_) => InsertMode::Value,
    }
}

fn insert_value_text(value: Option<&TableInsertValue>) -> Option<&str> {
    match value {
        Some(TableInsertValue::Value(value)) => Some(value),
        Some(TableInsertValue::Default | TableInsertValue::Null) | None => None,
    }
}

fn boolean_selection(value: Option<&TableInsertValue>) -> u32 {
    (insert_value_text(value) == Some("true")) as u32
}

fn insert_request_matches(
    object: Option<&DatabaseObject>,
    columns: &[TableColumn],
    request: &InsertRowRequest,
) -> bool {
    object == Some(&request.object) && columns == request.columns
}

fn row_grid() -> gtk::Grid {
    gtk::Grid::builder()
        .column_spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build()
}

fn column_label(column: &TableColumn, subtitle: String) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 1);
    container.set_width_request(COLUMN_LABEL_WIDTH);
    container.set_hexpand(false);

    let name = gtk::Label::builder()
        .label(&column.name)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(26)
        .tooltip_text(&column.name)
        .xalign(0.0)
        .build();
    name.add_css_class("heading");

    let subtitle = gtk::Label::builder()
        .label(&subtitle)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(30)
        .tooltip_text(&subtitle)
        .xalign(0.0)
        .build();
    subtitle.add_css_class("caption");
    subtitle.add_css_class("dim-label");

    container.append(&name);
    container.append(&subtitle);

    container
}

fn column_subtitle(column: &TableColumn) -> String {
    let mut parts = vec![column.display_type.clone()];
    if column.is_required_for_insert() {
        parts.push(gettext("Required"));
    } else if column.has_default && column.is_nullable {
        parts.push(gettext("Default or NULL allowed"));
    } else if column.has_default {
        parts.push(gettext("Default allowed"));
    } else if column.is_nullable {
        parts.push(gettext("NULL allowed"));
    }

    parts.join(" · ")
}

fn readonly_reason(column: &TableColumn) -> String {
    if column.is_required_for_insert() {
        return gettext("Required column with unsupported type");
    }
    if column.is_generated {
        return gettext("Generated column");
    }
    if column.is_identity {
        return gettext("Identity column");
    }
    if !column.is_editable_value_type() {
        return gettext("Unsupported column type");
    }

    gettext("Read-only column")
}

fn value_placeholder(column: &TableColumn) -> String {
    if column.is_required_for_insert() {
        gettext("Required value")
    } else {
        gettext("Value")
    }
}

fn sync_insert_response(
    dialog: &adw::AlertDialog,
    columns: &[TableColumn],
    inputs: &[InsertColumnInput],
) {
    dialog.set_response_enabled("insert", insert_values_are_valid(columns, inputs));
}

fn insert_values_are_valid(columns: &[TableColumn], inputs: &[InsertColumnInput]) -> bool {
    if columns
        .iter()
        .any(|column| !column.is_insertable() && column.is_required_for_insert())
    {
        return false;
    }

    inputs
        .iter()
        .all(|input| columns.get(input.column_index).is_some())
}

fn insert_values(columns: &[TableColumn], inputs: &[InsertColumnInput]) -> Vec<TableInsertValue> {
    let mut values = vec![TableInsertValue::Default; columns.len()];

    for input in inputs {
        let value = match input.mode() {
            InsertMode::Value => TableInsertValue::Value(input.value.value()),
            InsertMode::Null => TableInsertValue::Null,
            InsertMode::Default => TableInsertValue::Default,
        };

        values[input.column_index] = value;
    }

    values
}

pub(super) fn can_duplicate_row(columns: &[TableColumn], row: &[TableCell]) -> bool {
    columns.len() == row.len()
        && columns.iter().zip(row).all(|(column, cell)| {
            if column.is_identity
                || column.is_generated
                || column.is_primary_key && column.has_default
            {
                return true;
            }

            column.is_insertable() && (cell.is_null || duplicate_value_is_round_trip_safe(column))
        })
}

fn duplicate_values(columns: &[TableColumn], row: &[TableCell]) -> Option<Vec<TableInsertValue>> {
    if !can_duplicate_row(columns, row) {
        return None;
    }

    let values = columns
        .iter()
        .zip(row)
        .map(|(column, cell)| {
            if column.is_identity
                || column.is_generated
                || column.has_default && column.is_primary_key
            {
                TableInsertValue::Default
            } else if cell.is_null {
                TableInsertValue::Null
            } else {
                TableInsertValue::Value(cell.value.clone())
            }
        })
        .collect();

    Some(values)
}

fn duplicate_value_is_round_trip_safe(column: &TableColumn) -> bool {
    let type_name = column.type_name.to_ascii_lowercase();

    type_name != "char"
        && (column.uses_text_display()
            || matches!(
                column.type_group,
                ColumnTypeGroup::Boolean
                    | ColumnTypeGroup::DateTime
                    | ColumnTypeGroup::Numeric
                    | ColumnTypeGroup::Text
            ))
}

#[cfg(test)]
mod tests {
    use super::{
        InsertMode, InsertRowKind, InsertRowRequest, boolean_selection, can_duplicate_row,
        duplicate_values, insert_mode_for_value, insert_request_matches, insert_value_text,
    };
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::table_browser::{ColumnTypeGroup, TableCell, TableColumn, TableInsertValue};

    #[test]
    fn restores_insert_modes_from_saved_values() {
        assert_eq!(
            insert_mode_for_value(&TableInsertValue::Default),
            InsertMode::Default
        );
        assert_eq!(
            insert_mode_for_value(&TableInsertValue::Null),
            InsertMode::Null
        );
        assert_eq!(
            insert_mode_for_value(&TableInsertValue::Value("value".to_string())),
            InsertMode::Value
        );
    }

    #[test]
    fn restores_text_values_from_saved_values() {
        assert_eq!(
            insert_value_text(Some(&TableInsertValue::Value("value".to_string()))),
            Some("value")
        );
        assert_eq!(insert_value_text(Some(&TableInsertValue::Default)), None);
        assert_eq!(insert_value_text(Some(&TableInsertValue::Null)), None);
    }

    #[test]
    fn restores_boolean_selection_from_saved_values() {
        assert_eq!(
            boolean_selection(Some(&TableInsertValue::Value("true".to_string()))),
            1
        );
        assert_eq!(
            boolean_selection(Some(&TableInsertValue::Value("false".to_string()))),
            0
        );
        assert_eq!(boolean_selection(Some(&TableInsertValue::Null)), 0);
    }

    #[test]
    fn rejects_insert_requests_for_changed_columns() {
        let object = DatabaseObject {
            schema: "public".to_string(),
            name: "users".to_string(),
            kind: DatabaseObjectKind::Table,
        };
        let column = TableColumn {
            name: "name".to_string(),
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
        };
        let request = InsertRowRequest {
            kind: InsertRowKind::Insert,
            object: object.clone(),
            columns: vec![column.clone()],
            values: vec![TableInsertValue::Value("Ada".to_string())],
        };

        assert!(insert_request_matches(
            Some(&object),
            std::slice::from_ref(&column),
            &request
        ));
        assert!(!insert_request_matches(Some(&object), &[], &request));
    }

    #[test]
    fn duplicates_values_and_preserves_natural_primary_keys() {
        let columns = [
            test_column("id", true, false, false),
            test_column("name", false, false, false),
            test_column("note", false, false, false),
            test_column("generated", false, true, false),
            test_column("identity", true, false, true),
        ];
        let row = [
            TableCell::new("42".to_string()),
            TableCell::new("Ada".to_string()),
            TableCell::null(),
            TableCell::new("ignored".to_string()),
            TableCell::new("99".to_string()),
        ];

        assert_eq!(
            duplicate_values(&columns, &row),
            Some(vec![
                TableInsertValue::Value("42".to_string()),
                TableInsertValue::Value("Ada".to_string()),
                TableInsertValue::Null,
                TableInsertValue::Default,
                TableInsertValue::Default,
            ])
        );
    }

    #[test]
    fn rejects_binary_values_and_mismatched_rows() {
        let mut binary_column = test_column("payload", false, false, false);
        binary_column.type_group = ColumnTypeGroup::Binary;
        binary_column.type_name = "bytea".to_string();

        let null_row = [TableCell::null()];

        assert!(!can_duplicate_row(&[binary_column.clone()], &null_row));
        assert_eq!(duplicate_values(&[binary_column], &null_row), None);
        assert!(!can_duplicate_row(&[], &null_row));
    }

    #[test]
    fn allows_default_backed_binary_primary_keys() {
        let mut column = test_column("id", true, false, false);
        column.type_group = ColumnTypeGroup::Binary;
        column.type_name = "bytea".to_string();
        column.has_default = true;

        let row = [TableCell::new("ignored".to_string())];

        assert!(can_duplicate_row(std::slice::from_ref(&column), &row));
        assert_eq!(
            duplicate_values(&[column], &row),
            Some(vec![TableInsertValue::Default])
        );
    }

    #[test]
    fn accepts_datetime_values() {
        for type_name in ["date", "time", "timetz", "timestamp", "timestamptz"] {
            let mut column = test_column("starts_at", false, false, false);
            column.type_group = ColumnTypeGroup::DateTime;
            column.type_name = type_name.to_string();

            let row = [TableCell::new("12:00:00".to_string())];

            assert!(can_duplicate_row(&[column], &row));
        }
    }

    #[test]
    fn accepts_money_values() {
        let mut column = test_column("amount", false, false, false);
        column.type_group = ColumnTypeGroup::Numeric;
        column.type_name = "money".to_string();

        let row = [TableCell::new("1.234,56 €".to_string())];

        assert!(can_duplicate_row(&[column], &row));
    }

    #[test]
    fn rejects_postgres_internal_char_values() {
        let mut column = test_column("internal", false, false, false);
        column.type_name = "char".to_string();

        let row = [TableCell::new("\u{80}".to_string())];

        assert!(!can_duplicate_row(&[column], &row));
    }

    #[test]
    fn accepts_array_and_range_values() {
        let mut array = test_column("dates", false, false, false);
        array.type_group = ColumnTypeGroup::Other;
        array.type_name = "_date".to_string();
        array.display_type = "date[]".to_string();
        array.is_array = true;
        let mut range = test_column("period", false, false, false);
        range.type_group = ColumnTypeGroup::Other;
        range.type_name = "daterange".to_string();
        range.is_range = true;
        let row = [
            TableCell::new("{2024-01-02,2024-02-03}".to_string()),
            TableCell::new("[2024-01-02,2024-02-03)".to_string()),
        ];

        assert!(can_duplicate_row(&[array.clone(), range.clone()], &row));
        assert_eq!(
            duplicate_values(&[array, range], &row),
            Some(vec![
                TableInsertValue::Value("{2024-01-02,2024-02-03}".to_string()),
                TableInsertValue::Value("[2024-01-02,2024-02-03)".to_string()),
            ])
        );
    }

    fn test_column(
        name: &str,
        is_primary_key: bool,
        is_generated: bool,
        is_identity: bool,
    ) -> TableColumn {
        TableColumn {
            name: name.to_string(),
            display_type: "text".to_string(),
            type_name: "text".to_string(),
            enum_values: Vec::new(),
            type_group: ColumnTypeGroup::Text,
            is_array: false,
            is_range: false,
            is_nullable: true,
            is_primary_key,
            has_default: false,
            is_identity,
            is_generated,
            ordinal_position: 1,
        }
    }
}
