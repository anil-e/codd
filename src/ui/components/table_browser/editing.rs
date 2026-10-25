use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_browser::{
    ColumnTypeGroup, TableCell, TableCellChange, TableColumn, TablePage, TableRowEdit,
};
use crate::ui::components::table_browser::{
    TableBrowser, TableBrowserCommandOutput, TableBrowserMsg, TableBrowserOutput, close_popover,
};

use super::cell_editor::show_edit_cell_popover;

impl TableBrowser {
    pub fn has_pending_edits(&self) -> bool {
        !self.pending_edits.is_empty()
    }

    pub fn edits_saving(&self) -> bool {
        self.save_request.is_some()
    }

    pub fn input_locked(&self) -> bool {
        self.is_loading
            || self.edits_saving()
            || self.close_request.is_some()
            || self.commit_unknown
    }

    pub fn authoritative_changes_blocked(&self) -> bool {
        self.has_pending_edits()
            || self.edits_saving()
            || self.close_request.is_some()
            || self.commit_unknown
            || self.active_insert_request_id.is_some()
            || self.active_delete_request_id.is_some()
    }

    pub(super) fn displayed_page(&self) -> Option<TablePage> {
        let mut page = self.page.clone()?;

        for (&(row, column), value) in &self.pending_edits {
            if let Some(cell) = page.rows.get_mut(row).and_then(|row| row.get_mut(column)) {
                *cell = value.clone();
            }
        }

        Some(page)
    }

    pub(super) fn pending_edits_label(&self) -> String {
        if self.commit_unknown {
            return gettext(
                "The database may have committed these changes. Do not retry. Discard and reload to check the database before editing again.",
            );
        }

        let (cells, rows) = pending_counts(&self.pending_edits);

        gettext("Changed cells: {cells}; affected rows: {rows}")
            .replace("{cells}", &cells.to_string())
            .replace("{rows}", &rows.to_string())
    }

    pub(super) fn reject_input(
        &mut self,
        msg: &TableBrowserMsg,
        sender: &ComponentSender<Self>,
    ) -> bool {
        if matches!(
            msg,
            TableBrowserMsg::ObjectRenamed(_) | TableBrowserMsg::SchemaChanged
        ) {
            if self.authoritative_changes_blocked() {
                match msg {
                    TableBrowserMsg::ObjectRenamed(object) => {
                        self.deferred_rename = Some(object.clone());
                    }
                    TableBrowserMsg::SchemaChanged => self.deferred_schema_change = true,
                    _ => unreachable!(),
                }

                return true;
            }

            return false;
        }

        if matches!(msg, TableBrowserMsg::Open { .. } | TableBrowserMsg::Refresh)
            && !self.authoritative_changes_blocked()
        {
            return false;
        }

        let replacement = matches!(
            msg,
            TableBrowserMsg::Open { .. }
                | TableBrowserMsg::ObjectRenamed(_)
                | TableBrowserMsg::Refresh
                | TableBrowserMsg::SchemaChanged
                | TableBrowserMsg::FirstPage
                | TableBrowserMsg::PreviousPage
                | TableBrowserMsg::NextPage
                | TableBrowserMsg::LastPage
                | TableBrowserMsg::PageSizeChanged(_)
                | TableBrowserMsg::ToggleFilters
                | TableBrowserMsg::FilterEvent(_)
                | TableBrowserMsg::SortChanged(_)
                | TableBrowserMsg::ExportCsvRequested
                | TableBrowserMsg::ExportCsvConfirmed { .. }
                | TableBrowserMsg::InsertRowRequested
                | TableBrowserMsg::DuplicateSelectedRowRequested
                | TableBrowserMsg::InsertRowConfirmed(_)
                | TableBrowserMsg::DeleteSelectedRowRequested
                | TableBrowserMsg::DeleteRowConfirmed { .. }
        );
        let editing = matches!(
            msg,
            TableBrowserMsg::EditCellRequested { .. }
                | TableBrowserMsg::EditCellFromMenu { .. }
                | TableBrowserMsg::CellEditSubmitted { .. }
        );

        if replacement && (self.has_pending_edits() || self.input_locked()) {
            if self.has_pending_edits() {
                let _ = sender.output(TableBrowserOutput::Copied(gettext(
                    "Commit or discard pending edits before continuing.",
                )));

                if let TableBrowserMsg::SortChanged(_) = msg {
                    super::sorting::sync_sort_indicator(&self.table_view, self.sort.as_ref());
                }
            }

            return true;
        }

        editing && self.input_locked()
    }

    pub(super) fn discard_edits(&mut self, sender: &ComponentSender<Self>) {
        self.page_generation = self.page_generation.wrapping_add(1);
        let was_dirty = self.has_pending_edits();

        self.pending_edits.clear();
        self.context_dirty.set(false);
        self.edits_error = None;
        self.commit_unknown = false;
        close_popover(&mut self.edit_popover);

        if was_dirty {
            let _ = sender.output(TableBrowserOutput::PendingEditsChanged(false));
        }
    }

    pub(super) fn save_edits(&mut self, sender: &ComponentSender<Self>) {
        if self.is_loading
            || self.edits_saving()
            || self.commit_unknown
            || !self.has_pending_edits()
        {
            return;
        }

        let (Some(pool), Some(page)) = (self.pool.clone(), self.page.as_ref()) else {
            self.edits_error = Some(gettext(
                "The table is no longer available. Discard and reload before editing.",
            ));

            if let Some(id) = self.close_request {
                self.cancel_edits_close(id, sender);
            }

            return;
        };

        let mut edits: Vec<TableRowEdit> = Vec::new();

        for (&(row_index, column_index), value) in &self.pending_edits {
            if edits.last().is_none_or(|edit| edit.row_index != row_index) {
                let (Some(original_row), Some(version)) =
                    (page.rows.get(row_index), page.row_versions.get(row_index))
                else {
                    self.edits_error = Some(gettext(
                        "The original row version is unavailable. Discard and reload before editing.",
                    ));

                    if let Some(id) = self.close_request {
                        self.cancel_edits_close(id, sender);
                    }

                    return;
                };

                edits.push(TableRowEdit {
                    row_index,
                    original_row: original_row.clone(),
                    version: version.clone(),
                    changes: Vec::new(),
                });
            }

            if let Some(edit) = edits.last_mut() {
                edit.changes.push(TableCellChange {
                    column_index,
                    value: value.clone(),
                });
            }
        }

        let object = page.object.clone();
        let columns = page.columns.clone();
        let generation = self.page_generation;
        let id = self.allocate_request_id();

        self.save_request = Some((id, generation));
        self.is_loading = true;
        self.context_busy.set(true);
        self.close_context_menu();
        close_popover(&mut self.edit_popover);
        let _ = sender.output(TableBrowserOutput::BusyChanged(true));

        sender.oneshot_command(async move {
            let result = db::browser::apply_table_edits(&pool, &object, &columns, &edits).await;

            TableBrowserCommandOutput::EditsSaved {
                id,
                generation,
                result,
            }
        });
    }

    pub(super) fn prepare_edits_close(
        &mut self,
        id: u64,
        root: &gtk::Box,
        sender: &ComponentSender<Self>,
    ) {
        if self.edits_saving()
            || self.close_request.is_some()
            || self.active_insert_request_id.is_some()
            || self.active_delete_request_id.is_some()
        {
            let _ = sender.output(TableBrowserOutput::EditsCloseCancelled(id));
            return;
        }

        self.close_request = Some(id);
        self.close_interrupted_read = self.is_loading;

        if let Some(abort) = self.active_abort_handle.take() {
            abort.abort();
        }

        self.active_request_id = None;
        self.active_last_page_request_id = None;
        self.is_loading = false;
        let _ = sender.output(TableBrowserOutput::BusyChanged(false));

        self.page_generation = self.page_generation.wrapping_add(1);
        self.close_discard = false;
        self.close_prepared = false;
        self.close_saved = false;
        self.close_committed = false;
        self.context_busy.set(true);
        self.close_context_menu();
        close_popover(&mut self.edit_popover);

        if !self.has_pending_edits() {
            self.close_prepared = true;
            let _ = sender.output(TableBrowserOutput::EditsClosePrepared(id));
            return;
        }

        let title = self
            .object
            .as_ref()
            .map(DatabaseObject::qualified_name)
            .unwrap_or_default();
        let description = if self.commit_unknown {
            self.pending_edits_label()
        } else {
            gettext("Commit the changes to the database, discard them, or cancel closing.")
        };

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Commit pending edits?"))
            .body(format!("{title}\n{description}"))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("discard", &gettext("Discard")),
            ("save", &gettext("Commit")),
        ]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_enabled("save", !self.commit_unknown);

        dialog.connect_response(None, {
            let input = sender.input_sender().clone();

            move |_, response| {
                let _ = input.send(TableBrowserMsg::EditsCloseResponse {
                    id,
                    response: response.to_string(),
                });
            }
        });

        dialog.present(root.root().and_downcast::<gtk::Window>().as_ref());
        self.close_dialog = Some(dialog);
    }

    pub(super) fn cancel_edits_close(&mut self, id: u64, sender: &ComponentSender<Self>) {
        if self.close_request == Some(id) {
            if let Some(backup) = self.close_backup.take() {
                let was_dirty = self.has_pending_edits();

                self.pending_edits = backup.pending_edits;
                self.edits_error = backup.error;
                self.commit_unknown = backup.commit_unknown;
                self.context_dirty.set(self.has_pending_edits());

                if was_dirty != self.has_pending_edits() {
                    let _ = sender.output(TableBrowserOutput::PendingEditsChanged(
                        self.has_pending_edits(),
                    ));
                }
            }

            self.close_request = None;
            self.close_discard = false;
            self.close_prepared = false;
            self.close_committed = false;

            if let Some(dialog) = self.close_dialog.take() {
                dialog.close();
            }

            self.context_busy
                .set(self.is_loading || self.commit_unknown);
            self.close_saved = false;
        }

        let _ = sender.output(TableBrowserOutput::EditsCloseCancelled(id));
    }

    pub(super) fn open_edit_popover(
        &mut self,
        anchor: &gtk::Label,
        row_index: usize,
        column_index: usize,
        sender: &ComponentSender<Self>,
        root: &gtk::Box,
    ) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let Some(column) = page.columns.get(column_index) else {
            return;
        };

        if page
            .row_versions
            .get(row_index)
            .is_none_or(String::is_empty)
        {
            self.show_warning(root, &gettext("Cell cannot be edited"), &gettext("This row has no usable version for safe editing. Reload the table and check its permissions and row-level security policy."));
            return;
        }

        let Some(original) = page
            .rows
            .get(row_index)
            .and_then(|row| row.get(column_index))
        else {
            return;
        };

        let cell = self
            .pending_edits
            .get(&(row_index, column_index))
            .unwrap_or(original);

        if let Err(error) = validate_editable_cell(&page.object, &page.columns, column_index) {
            self.show_warning(root, &gettext("Cell cannot be edited"), &error);
            return;
        }

        close_popover(&mut self.edit_popover);

        self.edit_popover = Some(show_edit_cell_popover(
            anchor,
            column,
            cell,
            sender.clone(),
            self.page_generation,
            row_index,
            column_index,
        ));
        if let Some(popover) = &self.edit_popover {
            let input = sender.input_sender().clone();
            popover.connect_visible_notify(move |_| {
                let _ = input.send(TableBrowserMsg::RefreshActivityChanged);
            });
        }
    }

    pub(super) fn stage_cell(
        &mut self,
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        value: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        if page_generation != self.page_generation || self.input_locked() {
            return;
        }

        close_popover(&mut self.edit_popover);

        let Some(page) = self.page.as_ref() else {
            return;
        };

        if validate_editable_cell(&page.object, &page.columns, column_index).is_err() {
            return;
        }

        if page
            .row_versions
            .get(row_index)
            .is_none_or(String::is_empty)
        {
            return;
        }
        let Some(original) = page
            .rows
            .get(row_index)
            .and_then(|row| row.get(column_index))
        else {
            return;
        };

        let was_dirty = self.has_pending_edits();
        let cell = submitted_cell(value, &page.columns[column_index]);
        stage_value(
            &mut self.pending_edits,
            (row_index, column_index),
            original,
            cell,
        );
        self.context_dirty.set(self.has_pending_edits());

        if !self.has_pending_edits() {
            self.edits_error = None;
        }

        if was_dirty != self.has_pending_edits() {
            let _ = sender.output(TableBrowserOutput::PendingEditsChanged(
                self.has_pending_edits(),
            ));
        }
    }
}

fn submitted_cell(value: Option<String>, column: &TableColumn) -> TableCell {
    let Some(value) = value else {
        return TableCell::null();
    };

    if column.type_group == ColumnTypeGroup::Boolean && !column.is_array {
        if value.eq_ignore_ascii_case("true") || value == "t" {
            return TableCell::new("true".into());
        }

        if value.eq_ignore_ascii_case("false") || value == "f" {
            return TableCell::new("false".into());
        }
    }

    TableCell::new(value)
}

fn validate_editable_cell(
    object: &DatabaseObject,
    columns: &[TableColumn],
    column_index: usize,
) -> Result<(), String> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(gettext("Only tables can be edited."));
    }

    let Some(column) = columns.get(column_index) else {
        return Err(gettext("The selected cell is no longer available."));
    };

    if !column.is_editable() {
        return Err(gettext("This column type is not editable yet."));
    }

    if !columns.iter().any(|column| column.is_primary_key) {
        return Err(gettext("Editing requires a primary key."));
    }

    Ok(())
}

fn stage_value(
    pending: &mut std::collections::BTreeMap<(usize, usize), TableCell>,
    key: (usize, usize),
    original: &TableCell,
    value: TableCell,
) {
    if &value == original {
        pending.remove(&key);
    } else {
        pending.insert(key, value);
    }
}

fn pending_counts(
    pending: &std::collections::BTreeMap<(usize, usize), TableCell>,
) -> (usize, usize) {
    (
        pending.len(),
        pending
            .keys()
            .map(|(row, _)| *row)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(type_name: &str) -> TableColumn {
        TableColumn {
            name: "value".into(),
            display_type: type_name.into(),
            type_name: type_name.into(),
            enum_values: Vec::new(),
            type_group: ColumnTypeGroup::from_postgres_type(type_name),
            is_array: false,
            is_range: false,
            is_nullable: true,
            is_primary_key: false,
            has_default: false,
            is_identity: false,
            is_generated: false,
            ordinal_position: 2,
        }
    }

    #[test]
    fn normalizes_boolean_choices_without_changing_text() {
        assert_eq!(
            submitted_cell(Some("TRUE".into()), &column("bool")),
            TableCell::new("true".into())
        );
        assert_eq!(
            submitted_cell(Some("FALSE".into()), &column("bool")),
            TableCell::new("false".into())
        );
        assert_eq!(
            submitted_cell(Some(" TRUE ".into()), &column("text")),
            TableCell::new(" TRUE ".into())
        );
        assert_eq!(
            submitted_cell(Some("NULL".into()), &column("text")),
            TableCell::new("NULL".into())
        );
        assert_eq!(submitted_cell(None, &column("text")), TableCell::null());
    }

    #[test]
    fn counts_distinct_cells_and_rows() {
        let mut pending = std::collections::BTreeMap::new();
        let original = TableCell::null();

        for key in [(0, 1), (0, 2), (3, 1)] {
            stage_value(
                &mut pending,
                key,
                &original,
                TableCell::new("edited".into()),
            );
        }

        assert_eq!(pending_counts(&pending), (3, 2));

        stage_value(&mut pending, (3, 1), &original, original.clone());
        assert_eq!(pending_counts(&pending), (2, 1));
    }

    #[test]
    fn staging_replaces_and_reverts_exact_values() {
        let mut pending = std::collections::BTreeMap::new();
        let original = TableCell::new("original".into());

        stage_value(
            &mut pending,
            (0, 1),
            &original,
            TableCell::new("first".into()),
        );
        stage_value(
            &mut pending,
            (0, 1),
            &original,
            TableCell::new("second".into()),
        );

        assert_eq!(pending.len(), 1);
        assert_eq!(pending[&(0, 1)].value, "second");

        stage_value(&mut pending, (0, 1), &original, original.clone());
        assert!(pending.is_empty());
    }

    #[test]
    fn staging_distinguishes_null_empty_and_literal_null() {
        let mut pending = std::collections::BTreeMap::new();
        let original = TableCell::null();

        for value in ["", "NULL", " null "] {
            stage_value(
                &mut pending,
                (0, 0),
                &original,
                TableCell::new(value.into()),
            );
            assert_eq!(pending[&(0, 0)], TableCell::new(value.into()));
        }

        stage_value(&mut pending, (0, 0), &original, TableCell::null());
        assert!(pending.is_empty());
    }
}
