use gettextrs::gettext;
use libadwaita as adw;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::models::connection::SavedConnection;
use crate::models::query_history::QueryHistoryEntry;
use crate::state::{connection_store, query_history_store};
use crate::ui::components::{
    database_selector::DatabaseSelectorMsg, editor::SqlEditorMsg, sidebar::ObjectSidebarMsg,
    start_screen::StartScreenMsg,
};

use super::{WindowContent, WindowContentCommandOutput, WindowContentWidgets};

impl WindowContent {
    pub(super) fn handle_databases_loaded(&mut self, id: u64, result: Result<Vec<String>, String>) {
        if self.active_database_list_request_id != Some(id) {
            return;
        }

        self.active_database_list_request_id = None;

        self.database_selector
            .emit(DatabaseSelectorMsg::SetLoading(false));

        match result {
            Ok(databases) => {
                let databases = self.databases_with_active_database(databases);
                self.state.available_databases = databases.clone();
                self.database_selector
                    .emit(DatabaseSelectorMsg::SetDatabases(databases));
            }
            Err(_) => {
                if let Some(database) = self.state.active_database.clone() {
                    self.state.available_databases = vec![database.clone()];
                    self.database_selector
                        .emit(DatabaseSelectorMsg::SetDatabases(vec![database]));
                }
            }
        }
    }

    pub(super) fn switch_database(
        &mut self,
        database: String,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.state.active_database.as_deref() == Some(database.as_str()) {
            return;
        }

        let Some(details) = self.active_connection_details.clone() else {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Connect to PostgreSQL before switching databases.",
            )));
            return;
        };

        if let Err(error) = self.save_current_session(widgets) {
            widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                "{}: {error}",
                gettext("Saving tabs failed")
            )));
        }

        let request_id = self.allocate_database_switch_request_id();
        self.active_database_switch_request_id = Some(request_id);
        self.database_selector
            .emit(DatabaseSelectorMsg::SetLoading(true));

        let target_details = details.with_database(database.clone());
        sender.oneshot_command(async move {
            WindowContentCommandOutput::DatabaseSwitched {
                id: request_id,
                database: database.clone(),
                result: db::postgres::connect_to_database(&target_details, &database)
                    .await
                    .map_err(|error| error.to_string()),
            }
        });
    }

    pub(super) fn handle_database_switched(
        &mut self,
        id: u64,
        database: String,
        result: Result<PgPool, String>,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.active_database_switch_request_id != Some(id) {
            return;
        }

        self.active_database_switch_request_id = None;

        let pool = match result {
            Ok(pool) => pool,
            Err(error) => {
                self.database_selector
                    .emit(DatabaseSelectorMsg::SetLoading(false));
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Database switch failed")
                )));
                return;
            }
        };

        self.cancel_all_queries();
        self.active_schema_request_id = None;
        self.active_pool = Some(pool);
        self.state.active_database = Some(database.clone());
        self.state.objects.clear();
        let Some(mut details) = self.active_connection_details.clone() else {
            return;
        };

        details.saved.database = database.clone();
        let connection = details.saved.clone();
        self.active_connection_details = Some(details);
        self.state.active_connection = Some(connection.clone());

        self.database_selector
            .emit(DatabaseSelectorMsg::SetContext {
                connection_title: connection.name.clone(),
                active_database: database,
                databases: self
                    .databases_with_active_database(self.state.available_databases.clone()),
            });
        self.database_selector
            .emit(DatabaseSelectorMsg::SetLoading(false));

        self.load_query_history(&connection);
        self.restore_saved_session_or_default(widgets, sender);
        self.sidebar.emit(ObjectSidebarMsg::Loading);
        self.reload_schema(sender);

        match connection_store::save_connection(&connection) {
            Ok(connections) => {
                self.state.connections.clone_from(&connections);
                self.start_screen
                    .emit(StartScreenMsg::SetConnections(connections.clone()));
                self.broadcast_connections_changed(connections);
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Saving the selected database failed")
                )));
            }
        }
    }

    pub(super) fn databases_with_active_database(&self, mut databases: Vec<String>) -> Vec<String> {
        let Some(active_database) = self.state.active_database.as_ref() else {
            return databases;
        };

        if !databases.iter().any(|database| database == active_database) {
            databases.insert(0, active_database.clone());
        }

        databases
    }

    pub(super) fn allocate_database_list_request_id(&mut self) -> u64 {
        let id = self.next_database_list_request_id;
        self.next_database_list_request_id = self.next_database_list_request_id.wrapping_add(1);
        id
    }

    pub(super) fn allocate_database_switch_request_id(&mut self) -> u64 {
        let id = self.next_database_switch_request_id;
        self.next_database_switch_request_id = self.next_database_switch_request_id.wrapping_add(1);
        id
    }

    pub(super) fn migrate_legacy_query_history(
        &self,
        connection: &SavedConnection,
        widgets: &WindowContentWidgets,
    ) {
        if let Err(error) = query_history_store::migrate_legacy_connection_history(
            &connection.id,
            &connection.database,
        ) {
            widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                "{}: {error}",
                gettext("Migrating query history failed")
            )));
        }
    }

    pub(super) fn load_query_history(&mut self, connection: &SavedConnection) {
        self.set_query_history(query_history_store::load_for_database(
            &connection.id,
            &connection.database,
        ));
    }

    pub(super) fn record_query_history(&mut self, widgets: &WindowContentWidgets, sql: &str) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        let Some(database) = self.state.active_database.as_ref() else {
            return;
        };

        match query_history_store::record_query_for_database(&connection.id, database, sql) {
            Ok(history) => {
                self.set_query_history(history);
                self.broadcast_query_history_changed();
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Saving query history failed")
                )));
            }
        }
    }

    pub(super) fn clear_query_history(&mut self, widgets: &WindowContentWidgets) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        let Some(database) = self.state.active_database.as_ref() else {
            return;
        };

        match query_history_store::clear_database(&connection.id, database) {
            Ok(history) => {
                self.set_query_history(history);
                self.broadcast_query_history_changed();
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Clearing query history failed")
                )));
            }
        }
    }

    fn set_query_history(&mut self, history: Vec<QueryHistoryEntry>) {
        self.query_history = history;

        for tab in &self.query_tabs {
            tab.editor
                .emit(SqlEditorMsg::SetHistory(self.query_history.clone()));
        }
    }

    pub(super) fn reload_query_history_if_active(&mut self, connection_id: &str, database: &str) {
        let Some(active_connection) = self.state.active_connection.as_ref() else {
            return;
        };

        if active_connection.id != connection_id
            || self.state.active_database.as_deref() != Some(database)
        {
            return;
        }

        self.set_query_history(query_history_store::load_for_database(
            connection_id,
            database,
        ));
    }
}
