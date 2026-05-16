use gettextrs::gettext;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_browser::{TableCell, TableColumn};
use crate::ui::components::table_browser::{
    TableBrowser, TableBrowserCommandOutput, close_popover,
};

use super::cell_editor::show_edit_cell_popover;

impl TableBrowser {
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
        let Some(cell) = page
            .rows
            .get(row_index)
            .and_then(|row| row.get(column_index))
        else {
            return;
        };

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
    }

    pub(super) fn update_cell(
        &mut self,
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        value: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        if page_generation != self.page_generation {
            return;
        }

        close_popover(&mut self.edit_popover);

        let (Some(pool), Some(page)) = (self.pool.clone(), self.page.clone()) else {
            return;
        };
        let Some(row) = page.rows.get(row_index).cloned() else {
            return;
        };

        sender.oneshot_command(async move {
            let result = db::browser::update_table_cell(
                &pool,
                &page.object,
                &page.columns,
                &row,
                column_index,
                value,
            )
            .await
            .map_err(|error| error.to_string());

            TableBrowserCommandOutput::CellUpdated {
                page_generation,
                row_index,
                column_index,
                result,
            }
        });
    }

    pub(super) fn handle_cell_updated(
        &mut self,
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        result: Result<TableCell, String>,
    ) -> Result<(), String> {
        if page_generation != self.page_generation {
            return Ok(());
        }

        let Some(page) = self.page.as_mut() else {
            return Ok(());
        };

        match result {
            Ok(cell) => {
                if let Some(row) = page.rows.get_mut(row_index)
                    && let Some(value) = row.get_mut(column_index)
                {
                    *value = cell;
                }

                Ok(())
            }

            Err(error) => Err(error),
        }
    }
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

    if column.is_primary_key {
        return Err(gettext("Primary key columns are read-only for now."));
    }

    if !column.is_editable_value_type() {
        return Err(gettext("This column type is not editable yet."));
    }

    if !columns.iter().any(|column| column.is_primary_key) {
        return Err(gettext("Editing requires a primary key."));
    }

    Ok(())
}
