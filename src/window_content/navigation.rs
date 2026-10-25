use libadwaita as adw;
use relm4::prelude::*;

use super::{WindowContent, WindowContentMsg, WindowContentWidgets};
use crate::db::postgres::PostgresConnection;
use crate::models::connection::ConnectionDetails;

#[derive(Default)]
pub(super) struct PendingNavigation {
    pub disconnect_pending: bool,
    pub deferred_switch: Option<WindowContentMsg>,
    pub deferred_connection: Option<(ConnectionDetails, PostgresConnection)>,
}

impl WindowContent {
    pub(super) fn resume_pending_navigation(
        &mut self,
        widgets: &mut WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if self.file_operation_pending() || self.close_preparation_pending() {
            return;
        }

        if self.pending_navigation.disconnect_pending {
            self.pending_navigation.deferred_switch = None;
            self.pending_navigation.deferred_connection = None;
            self.show_start_screen(widgets);
            return;
        }

        if let Some((details, postgres)) = self.pending_navigation.deferred_connection.take() {
            self.handle_connected(details, postgres, widgets, sender, root);
            return;
        }

        if let Some(msg) = self.pending_navigation.deferred_switch.take() {
            sender.input(msg);
            return;
        }

        self.resume_queued_sql_files(sender);
    }
}
