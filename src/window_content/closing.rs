use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::prelude::*;

use super::documents::{SqlDocument, document_error, present_document_dialog, query_id};
use super::{
    TableViewOutput, WindowContent, WindowContentMsg, WindowContentOutput, WindowContentWidgets,
};
use crate::ui::components::table_view::TableViewMsg;

#[derive(Default)]
pub(super) struct ClosingWorkspace {
    pub preparation: Option<ClosePreparation>,
    dialog: Option<adw::AlertDialog>,
    next_generation: u64,
}

#[derive(Default)]
pub(super) struct ClosePreparation {
    browse_remaining: Vec<u64>,
    browse_prepared: Vec<u64>,
    browse_waiting: Option<u64>,
    browse_committing: Vec<u64>,
    browse_cancelling: Vec<u64>,
    recovery_restored: bool,
    browse_commit_started: bool,
    targets: Option<Vec<String>>,
    pub remaining: Vec<u64>,
    discarded: Vec<u64>,
    pub approved: bool,
    pub generation: u64,
    committing: bool,
    pub cancelling: bool,
    committed: bool,
}

impl ClosePreparation {
    fn window_prepare_response(&self) -> Option<WindowContentOutput> {
        if self.cancelling {
            return None;
        }

        if self.targets.is_none() && self.approved && !self.committing {
            Some(WindowContentOutput::ClosePrepared)
        } else {
            Some(WindowContentOutput::CloseCancelled)
        }
    }

    fn start_cancellation(&mut self) -> Option<u64> {
        if self.cancelling {
            return None;
        }

        self.cancelling = true;
        Some(self.generation)
    }

    fn accepts_cleanup(&self, generation: u64) -> bool {
        self.generation == generation && !self.cancelling
    }

    pub(super) fn accepts_decision(&self, generation: u64, tab: u64) -> bool {
        !self.approved
            && self.browse_remaining.is_empty()
            && self.browse_waiting.is_none()
            && !self.committing
            && !self.cancelling
            && self.generation == generation
            && self.remaining.last() == Some(&tab)
    }
}

#[derive(Debug)]
pub enum ClosingMsg {
    CloseDecision {
        generation: u64,
        tab: u64,
        decision: String,
    },
    CleanupFinished {
        generation: u64,
        result: Result<(), String>,
    },
    CancellationFinished {
        generation: u64,
        result: Result<(), String>,
    },
}

impl WindowContent {
    pub fn close_preparation_pending(&self) -> bool {
        self.closing.preparation.is_some()
    }

    pub(super) fn table_close_output(
        &mut self,
        tab: u64,
        output: TableViewOutput,
        widgets: &mut WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let Some(close) = self.closing.preparation.as_mut() else {
            return;
        };

        match output {
            TableViewOutput::EditsClosePrepared(id)
                if id == close.generation
                    && close.browse_waiting == Some(tab)
                    && !close.cancelling
                    && !close.committing =>
            {
                close.browse_waiting = None;
                close.browse_prepared.push(tab);

                self.advance_close(widgets, root, sender);
            }
            TableViewOutput::EditsCloseCancelled(id) if id == close.generation => {
                if close.cancelling && close.browse_cancelling.contains(&tab) {
                    close.browse_cancelling.retain(|item| *item != tab);

                    if close.browse_cancelling.is_empty() && close.recovery_restored {
                        sender.input(WindowContentMsg::Closing(
                            ClosingMsg::CancellationFinished {
                                generation: id,
                                result: Ok(()),
                            },
                        ));
                    }
                } else if close.browse_waiting == Some(tab)
                    || close.browse_committing.contains(&tab)
                {
                    self.cancel_close(root, sender);
                }
            }
            TableViewOutput::EditsCloseCommitted(id)
                if id == close.generation
                    && close.committing
                    && !close.cancelling
                    && close.browse_committing.contains(&tab) =>
            {
                close.browse_committing.retain(|item| *item != tab);

                if close.browse_committing.is_empty() {
                    sender.input(WindowContentMsg::Closing(ClosingMsg::CleanupFinished {
                        generation: id,
                        result: Ok(()),
                    }));
                }
            }
            _ => {}
        }
    }

    pub(super) fn close_targets(&self, keep: Option<&str>) -> Vec<String> {
        self.query_tabs
            .iter()
            .map(|tab| format!("query-tab-{}", tab.id))
            .chain(
                self.browse_tabs
                    .iter()
                    .map(|tab| format!("browse-tab-{}", tab.id)),
            )
            .filter(|name| Some(name.as_str()) != keep)
            .collect()
    }

    pub(super) fn begin_close(
        &mut self,
        targets: Option<Vec<String>>,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(close) = &self.closing.preparation {
            if targets.is_none() && close.targets.is_some() {
                self.cancel_close(root, sender);
                return;
            }

            if targets.is_none()
                && let Some(output) = close.window_prepare_response()
            {
                let _ = sender.output(output);
            }

            return;
        }

        if targets.as_ref().is_some_and(|names| names.len() == 1)
            && self.tab_count() == 1
            && self.query_tabs.len() == 1
            && self.query_tabs.iter().all(|tab| tab.document.is_none())
        {
            return;
        }

        if self.file_operation_pending() || self.active_database_switch_request_id.is_some() {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext("Wait for the current file or database operation to finish, then try closing again.")));
            let _ = sender.output(WindowContentOutput::CloseCancelled);
            return;
        }

        let remaining = self
            .query_tabs
            .iter()
            .filter(|tab| {
                targets
                    .as_ref()
                    .is_none_or(|names| names.contains(&format!("query-tab-{}", tab.id)))
                    && tab.document.as_ref().is_some_and(|doc| {
                        doc.is_dirty(&self.query_tab_sql(tab.id).unwrap_or_default())
                    })
            })
            .map(|tab| tab.id)
            .collect();
        self.closing.next_generation = self.closing.next_generation.wrapping_add(1);
        let generation = self.closing.next_generation;

        self.closing.preparation = Some(ClosePreparation {
            browse_remaining: self
                .browse_tabs
                .iter()
                .filter(|tab| {
                    tab.view.is_some()
                        && targets
                            .as_ref()
                            .is_none_or(|names| names.contains(&format!("browse-tab-{}", tab.id)))
                })
                .map(|tab| tab.id)
                .collect(),
            targets,
            remaining,
            generation,
            ..ClosePreparation::default()
        });

        for tab in &self.query_tabs {
            self.persist_document(tab.id, sender);
        }

        root.set_sensitive(false);
        self.advance_close(widgets, root, sender);
    }

    pub(super) fn advance_close(
        &mut self,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let Some(close) = &mut self.closing.preparation else {
            return;
        };

        if close.cancelling || close.committing || close.committed {
            return;
        }

        if close.browse_waiting.is_some() {
            return;
        }

        if let Some(id) = close.browse_remaining.pop() {
            close.browse_waiting = Some(id);

            if let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == id) {
                widgets.query_tab_view.set_selected_page(&tab.page);

                if let Some(view) = &tab.view {
                    view.emit(TableViewMsg::PrepareEditsClose(close.generation));
                }
            }

            return;
        }

        if let Some(&id) = close.remaining.last() {
            let generation = close.generation;
            let title = self
                .query_tabs
                .iter()
                .find(|tab| tab.id == id)
                .and_then(|tab| tab.document.as_ref())
                .map(SqlDocument::title)
                .unwrap_or_default();

            let dialog = adw::AlertDialog::builder()
                .heading(gettext("Save Changes?"))
                .body(format!(
                    "{}\n{}",
                    title,
                    gettext("Unsaved changes will be lost if you discard them.")
                ))
                .build();
            dialog.add_responses(&[
                ("cancel", &gettext("Cancel")),
                ("discard", &gettext("Discard")),
                ("save", &gettext("Save")),
            ]);
            dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("save"));
            dialog.set_close_response("cancel");

            let sender = sender.input_sender().clone();
            dialog.connect_response(None, move |_, decision| {
                let _ = sender.send(WindowContentMsg::Closing(ClosingMsg::CloseDecision {
                    generation,
                    tab: id,
                    decision: decision.into(),
                }));
            });

            self.closing.dialog = Some(dialog.clone());
            present_document_dialog(&dialog, root);
        } else {
            close.approved = true;

            if close.targets.is_none() {
                let _ = sender.output(WindowContentOutput::ClosePrepared);
            } else {
                self.commit_close(widgets, root, sender);
            }
        }
    }

    pub(super) fn cancel_close(&mut self, root: &adw::ToolbarView, sender: &ComponentSender<Self>) {
        let Some(close) = &mut self.closing.preparation else {
            return;
        };
        let Some(generation) = close.start_cancellation() else {
            return;
        };

        close.browse_cancelling = close.browse_prepared.clone();
        close.browse_cancelling.extend(close.browse_waiting);

        for id in &close.browse_cancelling {
            if let Some(view) = self
                .browse_tabs
                .iter()
                .find(|tab| tab.id == *id)
                .and_then(|tab| tab.view.as_ref())
            {
                view.emit(TableViewMsg::CancelEditsClose(generation));
            }
        }

        if let Some(dialog) = self.closing.dialog.take() {
            dialog.close();
        }

        self.cancel_document_chooser();
        root.set_sensitive(false);
        self.restore_close_recovery(generation, sender);
    }

    pub(super) fn commit_close(
        &mut self,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let Some(close) = &self.closing.preparation else {
            let _ = sender.output(WindowContentOutput::CloseCancelled);
            return;
        };

        if close.committed && !close.cancelling {
            let _ = sender.output(WindowContentOutput::CloseCommitted);
            return;
        }

        if !close.approved || close.committing || close.cancelling {
            return;
        }

        if let Err(error) = self.save_close_session(close.targets.as_deref(), widgets) {
            self.cancel_close(root, sender);
            document_error(root, &gettext("Saving Tabs Failed"), &error);
            return;
        }

        let close = self.closing.preparation.as_mut().expect("prepared close");
        close.committing = true;

        let close = self.closing.preparation.as_ref().expect("prepared close");
        self.remove_close_recovery(close.targets.as_deref(), close.generation, sender);
    }

    pub(super) fn handle_closing_message(
        &mut self,
        msg: ClosingMsg,
        widgets: &mut WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        match msg {
            ClosingMsg::CloseDecision {
                generation,
                tab,
                decision,
            } => {
                let Some(close) = &mut self.closing.preparation else {
                    return;
                };
                if !close.accepts_decision(generation, tab) {
                    return;
                }

                self.closing.dialog = None;
                match decision.as_str() {
                    "discard" => {
                        close.discarded.push(tab);
                        close.remaining.pop();
                        self.advance_close(widgets, root, sender);
                    }
                    "save" => self.save_document(tab, false, root, sender),
                    _ => self.cancel_close(root, sender),
                }
            }
            ClosingMsg::CleanupFinished { generation, result } => {
                let Some(close) = &self.closing.preparation else {
                    return;
                };
                if !close.accepts_cleanup(generation) {
                    return;
                }

                if !close.committing || close.committed || !close.browse_committing.is_empty() {
                    return;
                }

                if let Err(error) = result {
                    if let Some(close) = &mut self.closing.preparation {
                        close.committing = false;
                    }
                    self.cancel_close(root, sender);
                    document_error(root, &gettext("Closing SQL Files Failed"), &error);
                    return;
                }

                let close = self.closing.preparation.as_mut().expect("prepared close");

                if !close.browse_commit_started && !close.browse_prepared.is_empty() {
                    close.browse_commit_started = true;
                    close.browse_committing = close.browse_prepared.clone();

                    for id in &close.browse_committing {
                        if let Some(view) = self
                            .browse_tabs
                            .iter()
                            .find(|tab| tab.id == *id)
                            .and_then(|tab| tab.view.as_ref())
                        {
                            view.emit(TableViewMsg::CommitEditsClose(generation));
                        }
                    }

                    return;
                }

                let close = self.closing.preparation.take().expect("prepared close");
                if let Some(names) = close.targets {
                    if names.len() >= self.tab_count() {
                        self.add_query_tab(widgets, sender);
                    }
                    for name in names {
                        if let Some(id) = query_id(Some(&name)) {
                            self.release_document_tab(id);
                            self.close_query_tab(id, widgets);
                        } else if let Some(id) = name
                            .strip_prefix("browse-tab-")
                            .and_then(|id| id.parse().ok())
                        {
                            self.close_browse_tab(id, widgets);
                        }
                    }
                    root.set_sensitive(true);
                    self.schedule_session_save(sender);
                } else {
                    self.closing.preparation = Some(ClosePreparation {
                        committed: true,
                        ..close
                    });
                    let _ = sender.output(WindowContentOutput::CloseCommitted);
                }
            }
            ClosingMsg::CancellationFinished { generation, result } => {
                if !self
                    .closing
                    .preparation
                    .as_ref()
                    .is_some_and(|close| close.generation == generation && close.cancelling)
                {
                    return;
                }

                let close = self.closing.preparation.as_mut().expect("cancelled close");
                close.recovery_restored = true;

                if let Err(error) = &result {
                    document_error(root, &gettext("Restoring SQL Recovery Failed"), error);
                }

                if !close.browse_cancelling.is_empty() {
                    return;
                }

                self.closing.preparation = None;
                root.set_sensitive(true);
                let _ = sender.output(WindowContentOutput::CloseCancelled);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_decisions_are_serial_and_cannot_mutate_target_set() {
        let targets = vec!["query-tab-2".into(), "query-tab-3".into()];
        let mut close = ClosePreparation {
            targets: Some(targets.clone()),
            remaining: vec![2, 3],
            generation: 19,
            ..ClosePreparation::default()
        };
        assert!(!close.accepts_decision(18, 3));
        assert!(!close.accepts_decision(19, 2));
        assert!(close.accepts_decision(19, 3));

        close.browse_waiting = Some(9);

        assert!(!close.accepts_decision(19, 3));
        assert!(!close.accepts_decision(18, 3));

        close.browse_waiting = None;
        close.discarded.push(close.remaining.pop().unwrap());

        assert!(!close.accepts_decision(19, 3));
        assert!(close.accepts_decision(19, 2));
        assert_eq!(close.targets.as_ref(), Some(&targets));
        assert!(!close.approved);
        close.approved = true;
        assert!(!close.accepts_decision(19, 2));
    }

    #[test]
    fn cancellation_invalidates_cleanup_and_late_save_decisions_even_after_commit() {
        for committed in [false, true] {
            let targets = vec!["query-tab-5".into()];
            let mut close = ClosePreparation {
                targets: Some(targets.clone()),
                remaining: vec![5],
                approved: true,
                generation: 20,
                committing: true,
                committed,
                ..ClosePreparation::default()
            };
            assert!(close.accepts_cleanup(20));
            assert_eq!(close.start_cancellation(), Some(20));
            assert!(!close.accepts_cleanup(20));
            assert!(!close.accepts_cleanup(19));
            assert!(!close.accepts_decision(20, 5));
            assert_eq!(close.start_cancellation(), None);
            assert_eq!(close.targets.as_ref(), Some(&targets));
            assert_eq!(close.remaining, vec![5]);
        }
    }

    #[test]
    fn window_prepare_during_tab_close_is_declined_not_silently_ignored() {
        let mut close = ClosePreparation {
            targets: Some(vec!["query-tab-5".into()]),
            remaining: vec![5],
            generation: 20,
            ..ClosePreparation::default()
        };
        assert!(matches!(
            close.window_prepare_response(),
            Some(WindowContentOutput::CloseCancelled)
        ));
        close.targets = None;
        close.remaining.clear();
        close.approved = true;
        assert!(matches!(
            close.window_prepare_response(),
            Some(WindowContentOutput::ClosePrepared)
        ));
        close.committing = true;
        assert!(matches!(
            close.window_prepare_response(),
            Some(WindowContentOutput::CloseCancelled)
        ));
        close.start_cancellation();
        assert!(close.window_prepare_response().is_none());
        assert!(close.cancelling);
    }

    #[test]
    fn queued_browser_preparation_blocks_document_decisions() {
        let mut close = ClosePreparation {
            browse_remaining: vec![8],
            remaining: vec![5],
            generation: 21,
            ..ClosePreparation::default()
        };
        assert!(!close.accepts_decision(21, 5));
        close.browse_waiting = close.browse_remaining.pop();
        assert!(!close.accepts_decision(21, 5));
        close
            .browse_prepared
            .push(close.browse_waiting.take().unwrap());
        assert!(close.accepts_decision(21, 5));
        close.start_cancellation();
        assert!(!close.accepts_decision(21, 5));
    }
}
