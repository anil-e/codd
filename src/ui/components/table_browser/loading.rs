use futures_util::future::{AbortHandle, Abortable};
use gettextrs::gettext;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::ui::components::table_browser::{
    TableBrowser, TableBrowserCommandOutput, TableBrowserWidgets,
};

use super::grid::render_table;
use super::{close_popover, set_stack_child};

impl TableBrowser {
    pub(super) fn load_page(
        &mut self,
        widgets: &TableBrowserWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let (Some(pool), Some(object)) = (self.pool.clone(), self.object.clone()) else {
            return;
        };

        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }

        close_popover(&mut self.edit_popover);

        self.active_last_page_request_id = None;
        self.is_loading = true;
        self.is_error = false;
        self.status_title = gettext("Loading rows");
        self.status_description = Some(gettext("Fetching the selected page from PostgreSQL."));
        self.page = None;
        render_table(self, sender);
        set_stack_child(widgets, false);

        let id = self.allocate_request_id();
        let offset = self.offset;
        let page_size = self.page_size;
        let filters = self.active_filters.clone();
        let sort = self.sort.clone();
        self.active_request_id = Some(id);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.active_abort_handle = Some(abort_handle);

        sender.oneshot_command(async move {
            let load = async move {
                db::browser::load_table_page(
                    &pool,
                    &object,
                    offset,
                    page_size,
                    &filters,
                    sort.as_ref(),
                )
                .await
                .map_err(|error| table_load_error_message(&object, &error))
            };

            let result = match Abortable::new(load, abort_registration).await {
                Ok(result) => result,
                Err(_) => Err(gettext("Loading cancelled")),
            };

            TableBrowserCommandOutput::PageLoaded { id, result }
        });
    }

    pub(super) fn load_last_page_offset(&mut self, sender: &ComponentSender<Self>) {
        let (Some(pool), Some(object)) = (self.pool.clone(), self.object.clone()) else {
            return;
        };

        close_popover(&mut self.edit_popover);

        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }

        self.is_loading = true;
        let id = self.allocate_request_id();
        let page_size = self.page_size;
        let filters = self.active_filters.clone();
        self.active_last_page_request_id = Some(id);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.active_abort_handle = Some(abort_handle);

        sender.oneshot_command(async move {
            let count = async move {
                db::browser::load_table_row_count(&pool, &object, &filters)
                    .await
                    .map(|row_count| last_page_offset(row_count, page_size))
                    .map_err(|error| table_load_error_message(&object, &error))
            };

            let result = match Abortable::new(count, abort_registration).await {
                Ok(result) => result,
                Err(_) => Err(gettext("Loading cancelled")),
            };

            TableBrowserCommandOutput::LastPageOffsetLoaded { id, result }
        });
    }

    fn allocate_request_id(&mut self) -> u64 {
        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        id
    }
}

fn table_load_error_message(object: &DatabaseObject, error: &sqlx::Error) -> String {
    if is_missing_relation(error) {
        let name = format!("{}.{}", object.schema, object.name);

        return match object.kind {
            DatabaseObjectKind::Table => {
                gettext("The table {name} could not be found. It may have been renamed or dropped.")
                    .replace("{name}", &name)
            }
            DatabaseObjectKind::View => {
                gettext("The view {name} could not be found. It may have been renamed or dropped.")
                    .replace("{name}", &name)
            }
        };
    }

    error.to_string()
}

fn is_missing_relation(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(error) => matches!(error.code().as_deref(), Some("42P01" | "3F000")),
        _ => false,
    }
}

pub(super) fn last_page_offset(row_count: i64, page_size: u32) -> u32 {
    if row_count <= 0 || page_size == 0 {
        return 0;
    }

    let row_count = row_count as u64;
    let page_size = u64::from(page_size);
    let offset = ((row_count - 1) / page_size) * page_size;

    u32::try_from(offset).unwrap_or(u32::MAX)
}
