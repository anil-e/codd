use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::DatabaseObjectKind;
use crate::ui::components::table_browser::{
    DeleteRowResult, TableBrowser, TableBrowserCommandOutput, TableBrowserMsg, TableBrowserOutput,
};

impl TableBrowser {
    pub(super) fn open_delete_row_dialog(&self, root: &gtk::Box, sender: &ComponentSender<Self>) {
        if !self.can_delete_selected_row() {
            return;
        }

        let Some((row_index, _)) = self.selected_row() else {
            return;
        };
        let page_generation = self.page_generation;

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Delete Row?"))
            .body(gettext("This will permanently delete the selected row."))
            .close_response("cancel")
            .build();

        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("delete", &gettext("Delete")),
        ]);
        dialog.set_default_response(Some("cancel"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);

        let sender = sender.clone();
        dialog.choose(
            root.root().and_downcast::<gtk::Window>().as_ref(),
            None::<&gtk::gio::Cancellable>,
            move |response| {
                if response == "delete" {
                    sender.input(TableBrowserMsg::DeleteRowConfirmed {
                        page_generation,
                        row_index,
                    });
                }
            },
        );
    }

    pub(super) fn delete_row(
        &mut self,
        page_generation: u64,
        row_index: usize,
        sender: &ComponentSender<Self>,
    ) {
        if self.page_generation != page_generation {
            return;
        }

        let Some(pool) = self.pool.clone() else {
            return;
        };
        let Some(page) = self.page.clone() else {
            return;
        };
        if page.object.kind != DatabaseObjectKind::Table {
            return;
        }
        let Some(row) = page.rows.get(row_index).cloned() else {
            return;
        };

        self.close_context_menu();
        self.is_loading = true;
        self.context_busy.set(true);
        let _ = sender.output(TableBrowserOutput::BusyChanged(true));
        let _ = sender.output(TableBrowserOutput::SelectionChanged {
            can_delete: false,
            can_duplicate: false,
        });
        let id = self.allocate_request_id();
        self.active_delete_request_id = Some(id);
        let previous_page = page.rows.len() == 1;

        sender.oneshot_command(async move {
            if let Err(error) =
                db::browser::delete_table_row(&pool, &page.object, &page.columns, &row).await
            {
                return TableBrowserCommandOutput::RowDeleted {
                    id,
                    result: DeleteRowResult::DeleteFailed(error.to_string()),
                };
            }

            TableBrowserCommandOutput::RowDeleted {
                id,
                result: DeleteRowResult::Deleted { previous_page },
            }
        });
    }
}
