use std::collections::HashMap;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use relm4::{ComponentController, ComponentSender, Sender};

use crate::models::connection::SavedConnection;
use crate::models::structure_action::StructureActionTarget;
use crate::ui::components::database_selector::DatabaseSelectorMsg;
use crate::ui::components::start_screen::StartScreenMsg;

use super::structure_actions::StructureActionScope;
use super::{WindowContent, WindowContentMsg, WindowContentWidgets};

static WINDOW_CONTENT_SUBSCRIBERS: Mutex<Option<HashMap<u64, Sender<WindowContentMsg>>>> =
    Mutex::new(None);
static NEXT_WINDOW_CONTENT_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub enum WindowContentEvent {
    Connections(Vec<SavedConnection>),
    QueryHistory {
        connection_id: String,
        database: String,
    },
    Schema {
        connection_id: String,
        database: String,
    },
    StructureChanged {
        connection_id: String,
        database: String,
        target: StructureActionTarget,
    },
}

pub(super) fn with_window_content_subscribers<R>(
    f: impl FnOnce(&mut HashMap<u64, Sender<WindowContentMsg>>) -> R,
) -> R {
    let mut guard = WINDOW_CONTENT_SUBSCRIBERS.lock().expect("subscribers lock");
    let subscribers = guard.get_or_insert_with(HashMap::new);
    f(subscribers)
}

pub(super) fn subscribe_window_content(sender: Sender<WindowContentMsg>) -> u64 {
    let id = NEXT_WINDOW_CONTENT_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed);

    with_window_content_subscribers(|subscribers| {
        subscribers.insert(id, sender);
    });

    id
}

pub(super) fn unsubscribe_window_content(id: u64) {
    with_window_content_subscribers(|subscribers| {
        subscribers.remove(&id);
    });
}

pub(super) fn broadcast_window_content_event(event: WindowContentEvent, except: Option<u64>) {
    with_window_content_subscribers(|subscribers| {
        subscribers.retain(|id, sender| {
            if Some(*id) == except {
                return true;
            }

            sender
                .send(WindowContentMsg::WindowEvent(event.clone()))
                .is_ok()
        });
    });
}

impl WindowContent {
    pub(super) fn handle_window_event(
        &mut self,
        event: WindowContentEvent,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        match event {
            WindowContentEvent::Connections(connections) => {
                self.apply_external_connections(connections, widgets);
            }
            WindowContentEvent::QueryHistory {
                connection_id,
                database,
            } => {
                self.reload_query_history_if_active(&connection_id, &database);
            }
            WindowContentEvent::Schema {
                connection_id,
                database,
            } => {
                if self.matches_active_database(&connection_id, &database) {
                    self.reload_schema(sender);
                }
            }
            WindowContentEvent::StructureChanged {
                connection_id,
                database,
                target,
            } => {
                if self.matches_active_database(&connection_id, &database) {
                    self.reload_schema(sender);
                    self.reload_browse_tab_after_structure_change(&target);
                }
            }
        }
    }

    fn apply_external_connections(
        &mut self,
        connections: Vec<SavedConnection>,
        widgets: &mut WindowContentWidgets,
    ) {
        let active_connection_id = self
            .state
            .active_connection
            .as_ref()
            .map(|connection| connection.id.clone());

        if let Some(active_connection_id) = active_connection_id {
            let updated = connections
                .iter()
                .find(|connection| connection.id == active_connection_id);

            if let Some(updated) = updated {
                if let Some(active_connection) = self.state.active_connection.as_mut() {
                    active_connection.name.clone_from(&updated.name);
                    active_connection.save_password = updated.save_password;
                }

                if let Some(details) = self.active_connection_details.as_mut() {
                    details.saved.name.clone_from(&updated.name);
                    details.saved.save_password = updated.save_password;
                }

                let active_database = self
                    .state
                    .active_database
                    .clone()
                    .unwrap_or_else(|| updated.database.clone());
                self.database_selector
                    .emit(DatabaseSelectorMsg::SetContext {
                        connection_title: updated.name.clone(),
                        active_database,
                        databases: self
                            .databases_with_active_database(self.state.available_databases.clone()),
                    });
            } else if self.shows_workspace() {
                self.show_start_screen(widgets);
            }
        }

        self.state.connections.clone_from(&connections);
        self.start_screen
            .emit(StartScreenMsg::SetConnections(connections));
    }

    pub(super) fn broadcast_connections_changed(&self, connections: Vec<SavedConnection>) {
        broadcast_window_content_event(
            WindowContentEvent::Connections(connections),
            Some(self.sync_subscription_id),
        );
    }

    pub(super) fn broadcast_query_history_changed(&self) {
        let Some((connection_id, database)) = self.active_database_scope() else {
            return;
        };

        broadcast_window_content_event(
            WindowContentEvent::QueryHistory {
                connection_id,
                database,
            },
            Some(self.sync_subscription_id),
        );
    }

    pub(super) fn broadcast_schema_changed(&self) {
        let Some((connection_id, database)) = self.active_database_scope() else {
            return;
        };

        broadcast_window_content_event(
            WindowContentEvent::Schema {
                connection_id,
                database,
            },
            Some(self.sync_subscription_id),
        );
    }

    pub(super) fn broadcast_structure_changed(
        &self,
        scope: StructureActionScope,
        target: StructureActionTarget,
    ) {
        broadcast_window_content_event(
            WindowContentEvent::StructureChanged {
                connection_id: scope.connection_id,
                database: scope.database,
                target,
            },
            Some(self.sync_subscription_id),
        );
    }
}
