use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_browser::{ColumnTypeGroup, TableColumn, TableInsertValue};
use crate::ui::components::table_browser::{
    InsertRowRequest, InsertRowResult, TableBrowser, TableBrowserCommandOutput, TableBrowserMsg,
    TableBrowserOutput,
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
            self.show_warning(
                root,
                &gettext("Inserting row failed"),
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
        let _ = sender.output(TableBrowserOutput::InsertRunningChanged(true));

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
                result: InsertRowResult::Inserted,
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

    let heading = error
        .map(|_| {
            gettext("Inserting row into {name} failed").replace("{name}", &object.qualified_name())
        })
        .unwrap_or_else(|| gettext("Insert Row"));
    let body = error.map(str::to_string).unwrap_or_else(|| {
        gettext(
            "Enter values for the new row. Columns can also use DEFAULT or NULL when available.",
        )
    });

    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .extra_child(&scroller)
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("insert", &gettext("Insert")),
    ]);
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

#[cfg(test)]
mod tests {
    use super::{
        InsertMode, InsertRowRequest, boolean_selection, insert_mode_for_value,
        insert_request_matches, insert_value_text,
    };
    use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
    use crate::models::table_browser::{ColumnTypeGroup, TableColumn, TableInsertValue};

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
            is_nullable: false,
            is_primary_key: false,
            has_default: false,
            is_identity: false,
            is_generated: false,
            ordinal_position: 1,
        };
        let request = InsertRowRequest {
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
}
