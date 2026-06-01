use futures_util::future::{AbortHandle, Abortable};
use gettextrs::gettext;
use libadwaita as adw;
use relm4::prelude::*;

use crate::db;
use crate::models::query_result::{DEFAULT_QUERY_RESULT_ROW_LIMIT, QueryExecutionResult};
use crate::ui::components::editor::SqlEditorMsg;
use crate::ui::components::results::QueryResultsMsg;

use super::{WindowContent, WindowContentCommandOutput, WindowContentWidgets};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QueryState {
    Idle,
    Running,
}

#[derive(Debug)]
pub(super) struct RunningQuery {
    pub(super) id: u64,
    pub(super) abort_handle: AbortHandle,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct QueryExecutionContext {
    pub(super) has_multiple_statements: bool,
    pub(super) reload_schema_on_success: bool,
}

impl WindowContent {
    pub(super) fn handle_query_executed(
        &mut self,
        tab_id: u64,
        id: u64,
        context: QueryExecutionContext,
        result: Result<QueryExecutionResult, String>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if !self.is_active_query(tab_id, id) {
            return;
        }

        let should_reload_schema = {
            let Some(tab) = self.query_tab_mut(tab_id) else {
                return;
            };

            tab.active_query = None;
            tab.query_state = QueryState::Idle;
            tab.editor.emit(SqlEditorMsg::SetRunning(false));

            match result {
                Ok(result) => {
                    if context.has_multiple_statements && query_result_reached_row_limit(&result) {
                        widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                            "Row limit reached. Query execution stopped.",
                        )));
                    }
                    tab.results.emit(QueryResultsMsg::ShowResult(result));
                    context.reload_schema_on_success
                }
                Err(error) => {
                    tab.results.emit(QueryResultsMsg::ShowError(format!(
                        "{}: {error}",
                        gettext("Query failed")
                    )));
                    false
                }
            }
        };

        if should_reload_schema {
            self.reload_schema(sender);
            self.broadcast_schema_changed();
        }
    }

    pub(super) fn handle_query_cancelled(&mut self, tab_id: u64, id: u64) {
        if !self.is_active_query(tab_id, id) {
            return;
        }

        if let Some(tab) = self.query_tab_mut(tab_id) {
            tab.active_query = None;
            tab.query_state = QueryState::Idle;
            tab.editor.emit(SqlEditorMsg::SetRunning(false));
            tab.results.emit(QueryResultsMsg::Cancelled);
        }
    }

    pub(super) fn run_query_for_tab(
        &mut self,
        tab_id: u64,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.query_tabs.iter().all(|tab| tab.id != tab_id) {
            return;
        }

        if self
            .query_tab_mut(tab_id)
            .is_some_and(|tab| tab.query_state == QueryState::Running)
        {
            widgets
                .toast_overlay
                .add_toast(adw::Toast::new(&gettext("A query is already running.")));
            return;
        }

        let Some(pool) = self.active_pool.clone() else {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Connect to PostgreSQL before running a query.",
            )));
            return;
        };

        let sql = self.query_tab_execution_sql(tab_id).unwrap_or_default();
        if sql.trim().is_empty() {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Enter SQL before running a query.",
            )));
            return;
        }

        let row_limit = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map_or(DEFAULT_QUERY_RESULT_ROW_LIMIT, |tab| tab.row_limit);
        self.record_query_history(widgets, &sql);

        let context = QueryExecutionContext {
            has_multiple_statements: db::query::has_multiple_statements(&sql),
            reload_schema_on_success: db::query::changes_schema(&sql),
        };
        let id = self.allocate_query_id();
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        if let Some(tab) = self.query_tab_mut(tab_id) {
            tab.query_state = QueryState::Running;
            tab.editor.emit(SqlEditorMsg::SetRunning(true));
            tab.results.emit(QueryResultsMsg::Loading);
            tab.active_query = Some(RunningQuery { id, abort_handle });
        }

        sender.oneshot_command(async move {
            let query = async move {
                db::query::execute(&pool, &sql, row_limit)
                    .await
                    .map_err(|error| error.to_string())
            };

            match Abortable::new(query, abort_registration).await {
                Ok(result) => WindowContentCommandOutput::QueryExecuted {
                    tab_id,
                    id,
                    context,
                    result,
                },
                Err(_) => WindowContentCommandOutput::QueryCancelled { tab_id, id },
            }
        });
    }

    pub(super) fn cancel_query(&mut self, tab_id: u64) {
        let Some(tab) = self.query_tab_mut(tab_id) else {
            return;
        };

        let Some(active_query) = tab.active_query.take() else {
            return;
        };

        active_query.abort_handle.abort();
        tab.query_state = QueryState::Idle;
        tab.editor.emit(SqlEditorMsg::SetRunning(false));
        tab.results.emit(QueryResultsMsg::Cancelled);
    }

    pub(super) fn cancel_all_queries(&mut self) {
        for tab in &mut self.query_tabs {
            if let Some(active_query) = tab.active_query.take() {
                active_query.abort_handle.abort();
                tab.query_state = QueryState::Idle;
                tab.editor.emit(SqlEditorMsg::SetRunning(false));
                tab.results.emit(QueryResultsMsg::Cancelled);
            }
        }
    }

    fn allocate_query_id(&mut self) -> u64 {
        let id = self.next_query_id;
        self.next_query_id = self.next_query_id.wrapping_add(1);
        id
    }

    fn is_active_query(&self, tab_id: u64, id: u64) -> bool {
        self.query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.active_query.as_ref())
            .as_ref()
            .is_some_and(|query| query.id == id)
    }
}

fn query_result_reached_row_limit(result: &QueryExecutionResult) -> bool {
    matches!(result, QueryExecutionResult::Rows(result) if result.row_limit_reached)
}
