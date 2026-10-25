use std::time::Duration;

use gettextrs::gettext;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::models::table_browser::TablePage;

use super::grid::render_table;
use super::{TableBrowser, TableBrowserMsg, TableBrowserOutput, TableBrowserWidgets};

pub(super) fn valid_interval(seconds: Option<u32>) -> bool {
    seconds.is_none_or(|value| (5..=3600).contains(&value))
}

pub(super) fn current_completion(id: u64, active: Option<u64>, blocked: bool) -> bool {
    active == Some(id) && !blocked
}

fn schedule_refresh(seconds: Option<u32>, ready: bool, scheduled: bool) -> bool {
    seconds.is_some() && valid_interval(seconds) && ready && !scheduled
}

#[derive(Default)]
struct RefreshActivity {
    hidden: bool,
    disconnected: bool,
    pending: bool,
    closing: bool,
    popover: bool,
    busy: bool,
    unavailable: bool,
}

impl RefreshActivity {
    fn pause_reason(&self) -> Option<String> {
        let reason = if self.hidden {
            gettext("Automatic refresh paused: tab hidden or window minimized")
        } else if self.disconnected {
            gettext("Automatic refresh paused: disconnected")
        } else if self.pending {
            gettext("Automatic refresh paused: commit or discard pending edits")
        } else if self.closing {
            gettext("Automatic refresh paused: tab is closing")
        } else if self.popover {
            gettext("Automatic refresh paused: close the editor, menu, or dialog")
        } else if self.busy {
            gettext("Automatic refresh paused: database operation in progress")
        } else if self.unavailable {
            gettext("Automatic refresh paused: no page loaded")
        } else {
            return None;
        };

        Some(reason)
    }
}

impl TableBrowser {
    pub fn refresh_tooltip(&self) -> String {
        let status = if self.is_refreshing {
            gettext("Refreshing rows...")
        } else if let Some(seconds) = self.refresh_seconds {
            self.refresh_activity().pause_reason().unwrap_or_else(|| {
                gettext("Refresh every {seconds} seconds")
                    .replace("{seconds}", &seconds.to_string())
            })
        } else {
            gettext("Refresh rows (automatic refresh is off)")
        };

        if let Some(error) = &self.refresh_error {
            return format!(
                "{status}\n{}\n{error}",
                gettext("Refresh failed. Displayed rows may be stale.")
            );
        }

        status
    }

    pub(super) fn refresh_ready(&self) -> bool {
        self.refresh_activity().pause_reason().is_none()
    }

    fn refresh_activity(&self) -> RefreshActivity {
        RefreshActivity {
            hidden: !self.refresh_active,
            disconnected: self.pool.as_ref().is_none_or(PgPool::is_closed),
            pending: self.has_pending_edits() || self.commit_unknown,
            closing: self.close_request.is_some(),
            popover: self.context_popover.is_visible()
                || self.refresh_dialog_open.get()
                || self
                    .edit_popover
                    .as_ref()
                    .is_some_and(|popover| popover.is_visible()),
            busy: self.is_loading
                || self.edits_saving()
                || self.is_refreshing
                || self.active_request_id.is_some()
                || self.active_last_page_request_id.is_some()
                || self.active_insert_request_id.is_some()
                || self.active_delete_request_id.is_some(),
            unavailable: self.object.is_none() || self.page.is_none(),
        }
    }

    pub(super) fn stop_refresh_timer(&mut self) {
        self.refresh_timer_generation = self.refresh_timer_generation.wrapping_add(1);
        if let Some(source) = self.refresh_source.take()
            && self.refresh_timer_live.replace(false)
        {
            source.remove();
        }
    }

    pub(super) fn cancel_background_refresh(&mut self) {
        if self.is_refreshing {
            if let Some(abort) = self.active_abort_handle.take() {
                abort.abort();
            }
            self.active_request_id = None;
            self.is_refreshing = false;
        }
    }

    pub(super) fn sync_refresh(&mut self, sender: &ComponentSender<Self>) {
        if self.refresh_seconds.is_none() || !self.refresh_ready() {
            self.stop_refresh_timer();
        } else if schedule_refresh(
            self.refresh_seconds,
            self.refresh_ready(),
            self.refresh_source.is_some(),
        ) && let Some(seconds) = self.refresh_seconds
        {
            let generation = self.refresh_timer_generation;
            let input = sender.input_sender().clone();
            let live = self.refresh_timer_live.clone();
            live.set(true);
            self.refresh_source = Some(glib::timeout_add_local_once(
                Duration::from_secs(u64::from(seconds)),
                move || {
                    live.set(false);
                    let _ = input.send(TableBrowserMsg::AutoRefreshTick(generation));
                },
            ));
        }

        let state = (
            self.refresh_seconds,
            self.refresh_tooltip(),
            self.is_refreshing,
        );
        if self.refresh_last_state.as_ref() != Some(&state) {
            let _ = sender.output(TableBrowserOutput::RefreshStateChanged {
                seconds: state.0,
                status: state.1.clone(),
                refreshing: state.2,
            });
            self.refresh_last_state = Some(state);
        }
    }

    pub(super) fn refresh_page(
        &mut self,
        widgets: &TableBrowserWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.page.is_none() {
            self.load_page(widgets, sender);
        } else if self.refresh_ready() {
            self.load_page_request(widgets, sender, true);
        }
    }

    pub(super) fn finish_refresh(
        &mut self,
        result: Result<TablePage, String>,
        widgets: &TableBrowserWidgets,
        sender: &ComponentSender<Self>,
    ) {
        self.active_request_id = None;
        self.active_abort_handle = None;
        self.is_refreshing = false;

        match result {
            Ok(page) => {
                self.refresh_error = None;
                let columns_changed = self.available_columns != page.columns;
                if columns_changed
                    && (!self.active_filters.is_empty() || self.sort.is_some())
                    && self.available_columns.iter().any(|old| {
                        !page
                            .columns
                            .iter()
                            .any(|new| new.name == old.name && new.type_name == old.type_name)
                    })
                {
                    self.refresh_error = Some(gettext(
                        "The table structure changed. Clear filters or sorting and refresh again.",
                    ));
                    return;
                }
                if self.page.as_ref() != Some(&page) {
                    self.available_columns.clone_from(&page.columns);
                    self.page = Some(page);
                    self.page_generation = self.page_generation.wrapping_add(1);
                    render_table(self, sender);
                }
                if columns_changed {
                    self.rebuild_filters(widgets, sender);
                }
            }
            Err(error) => self.refresh_error = Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_opt_in_and_bounded() {
        assert!(valid_interval(None));
        for seconds in [5, 30, 3600] {
            assert!(valid_interval(Some(seconds)));
        }
        for seconds in [0, 4, 3601, u32::MAX] {
            assert!(!valid_interval(Some(seconds)));
        }
    }

    #[test]
    fn scheduling_requires_enabled_idle_visible_browser_without_backlog() {
        assert!(schedule_refresh(Some(5), true, false));
        assert!(!schedule_refresh(None, true, false));
        assert!(!schedule_refresh(Some(5), false, false));
        assert!(!schedule_refresh(Some(5), true, true));
        assert!(!schedule_refresh(Some(0), true, false));
    }

    #[test]
    fn cancelled_superseded_or_editing_requests_cannot_replace_rows() {
        assert!(current_completion(8, Some(8), false));
        assert!(!current_completion(8, None, false));
        assert!(!current_completion(8, Some(9), false));
        assert!(!current_completion(8, Some(8), true));
    }

    #[test]
    fn activity_pause_reasons_cover_each_lifecycle_hold_and_resume() {
        let holds = [
            RefreshActivity {
                hidden: true,
                ..Default::default()
            },
            RefreshActivity {
                disconnected: true,
                ..Default::default()
            },
            RefreshActivity {
                pending: true,
                ..Default::default()
            },
            RefreshActivity {
                closing: true,
                ..Default::default()
            },
            RefreshActivity {
                popover: true,
                ..Default::default()
            },
            RefreshActivity {
                busy: true,
                ..Default::default()
            },
            RefreshActivity {
                unavailable: true,
                ..Default::default()
            },
        ];
        let reasons: Vec<_> = holds
            .iter()
            .map(|activity| activity.pause_reason().unwrap())
            .collect();
        for (index, reason) in reasons.iter().enumerate() {
            assert!(!reason.is_empty());
            assert!(!reasons[..index].contains(reason));
            assert!(!schedule_refresh(
                Some(5),
                holds[index].pause_reason().is_none(),
                false
            ));
        }
        assert!(RefreshActivity::default().pause_reason().is_none());
        assert!(schedule_refresh(
            Some(5),
            RefreshActivity::default().pause_reason().is_none(),
            false
        ));
    }
}
