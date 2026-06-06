use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::models::structure_action::{
    StructureActionKind, StructureActionTarget, StructureDropMode,
};

use super::{WindowContent, WindowContentCommandOutput, WindowContentMsg, WindowContentWidgets};

#[derive(Debug, Clone)]
pub(crate) struct StructureActionScope {
    pub(crate) connection_id: String,
    pub(crate) database: String,
}

#[derive(Debug, Clone)]
pub(crate) enum StructureActionRequest {
    Rename {
        scope: StructureActionScope,
        pool: PgPool,
        target: StructureActionTarget,
        new_name: String,
    },
    Drop {
        scope: StructureActionScope,
        pool: PgPool,
        target: StructureActionTarget,
        mode: StructureDropMode,
    },
}

#[derive(Debug)]
pub(crate) struct StructureActionError {
    message: String,
    allows_cascade: bool,
}

#[derive(Debug, Clone)]
struct CascadeDropRecovery {
    scope: StructureActionScope,
    pool: PgPool,
    target: StructureActionTarget,
}

impl StructureActionRequest {
    fn scope(&self) -> &StructureActionScope {
        match self {
            Self::Rename { scope, .. } | Self::Drop { scope, .. } => scope,
        }
    }

    fn target(&self) -> &StructureActionTarget {
        match self {
            Self::Rename { target, .. } | Self::Drop { target, .. } => target,
        }
    }
}

pub(super) fn show_rename_structure_item_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<WindowContent>,
    scope: StructureActionScope,
    pool: PgPool,
    target: StructureActionTarget,
) {
    if !target.editable {
        return;
    }

    let entry = gtk::Entry::builder()
        .text(&target.name)
        .activates_default(true)
        .hexpand(true)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading(rename_heading(&target))
        .body(gettext("Choose a new name for this structure item."))
        .extra_child(&entry)
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("rename", &gettext("Rename")),
    ]);

    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    dialog.set_response_enabled("rename", false);

    entry.connect_changed({
        let dialog = dialog.clone();
        let current_name = target.name.clone();

        move |entry| {
            dialog.set_response_enabled(
                "rename",
                db::object_actions::normalize_new_object_name(&entry.text())
                    .is_some_and(|name| name != current_name),
            );
        }
    });

    let sender = sender.clone();

    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "rename" {
                sender.input(WindowContentMsg::StructureActionConfirmed(
                    StructureActionRequest::Rename {
                        scope,
                        pool,
                        target,
                        new_name: entry.text().to_string(),
                    },
                ));
            }
        },
    );
}

pub(super) fn show_drop_structure_item_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<WindowContent>,
    scope: StructureActionScope,
    pool: PgPool,
    target: StructureActionTarget,
) {
    if !target.editable {
        return;
    }

    let dialog = adw::AlertDialog::builder()
        .heading(drop_heading(&target))
        .body(drop_body(&target))
        .build();

    dialog.add_responses(&[("cancel", &gettext("Cancel")), ("drop", &gettext("Drop"))]);
    dialog.set_response_appearance("drop", adw::ResponseAppearance::Destructive);

    let sender = sender.clone();

    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "drop" {
                sender.input(WindowContentMsg::StructureActionConfirmed(
                    StructureActionRequest::Drop {
                        scope,
                        pool,
                        target,
                        mode: StructureDropMode::Restrict,
                    },
                ));
            }
        },
    );
}

impl WindowContent {
    pub(super) fn run_structure_action(
        &self,
        request: StructureActionRequest,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let request = match normalize_structure_action_request(request, widgets) {
            Some(request) => request,
            None => return,
        };

        sender.oneshot_command(async move {
            let result = match &request {
                StructureActionRequest::Rename {
                    pool,
                    target,
                    new_name,
                    ..
                } => db::structure_actions::rename_structure_item(pool, target, new_name)
                    .await
                    .map_err(structure_action_error),
                StructureActionRequest::Drop {
                    pool, target, mode, ..
                } => db::structure_actions::drop_structure_item(pool, target, *mode)
                    .await
                    .map_err(structure_action_error),
            };

            WindowContentCommandOutput::StructureActionFinished(request, result)
        });
    }

    pub(super) fn handle_structure_action_completed(
        &mut self,
        request: StructureActionRequest,
        result: Result<(), StructureActionError>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let target = request.target().clone();
        let scope = request.scope().clone();

        match result {
            Ok(()) => {
                if self.matches_active_database(&scope.connection_id, &scope.database) {
                    self.reload_browse_tab_after_structure_change(&target);
                    self.reload_schema(sender);
                }

                self.broadcast_structure_changed(scope, target);

                widgets
                    .toast_overlay
                    .add_toast(adw::Toast::new(&structure_action_success_message(&request)));
            }
            Err(error) => {
                show_structure_action_error_dialog(
                    widgets,
                    &structure_action_failure_message(&request),
                    &error.message,
                    cascade_target(&request, &error),
                    sender,
                );
            }
        }
    }
}

fn structure_action_error(error: sqlx::Error) -> StructureActionError {
    let allows_cascade = error
        .as_database_error()
        .and_then(|error| error.code())
        .as_deref()
        == Some("2BP01");

    StructureActionError {
        message: error.to_string(),
        allows_cascade,
    }
}

fn cascade_target(
    request: &StructureActionRequest,
    error: &StructureActionError,
) -> Option<CascadeDropRecovery> {
    let StructureActionRequest::Drop {
        scope,
        pool,
        target,
        mode,
    } = request
    else {
        return None;
    };

    if *mode == StructureDropMode::Cascade || !error.allows_cascade {
        return None;
    }

    Some(CascadeDropRecovery {
        scope: scope.clone(),
        pool: pool.clone(),
        target: target.clone(),
    })
}

fn normalize_structure_action_request(
    request: StructureActionRequest,
    widgets: &WindowContentWidgets,
) -> Option<StructureActionRequest> {
    match request {
        StructureActionRequest::Rename {
            scope,
            pool,
            target,
            new_name,
        } => {
            if !target.editable {
                return None;
            }

            let new_name = match db::object_actions::normalize_new_object_name(&new_name) {
                Some(name) => name,
                None => {
                    widgets
                        .toast_overlay
                        .add_toast(adw::Toast::new(&gettext("Enter a valid object name.")));
                    return None;
                }
            };

            if new_name == target.name {
                return None;
            }

            Some(StructureActionRequest::Rename {
                scope,
                pool,
                target,
                new_name,
            })
        }
        StructureActionRequest::Drop {
            scope,
            pool,
            target,
            mode,
        } => {
            if target.editable {
                Some(StructureActionRequest::Drop {
                    scope,
                    pool,
                    target,
                    mode,
                })
            } else {
                None
            }
        }
    }
}

fn rename_heading(target: &StructureActionTarget) -> String {
    format!(
        "{} {}",
        gettext("Rename"),
        structure_kind_label(target.kind)
    )
}

fn drop_heading(target: &StructureActionTarget) -> String {
    format!("{} {}", gettext("Drop"), structure_kind_label(target.kind))
}

fn drop_body(target: &StructureActionTarget) -> String {
    format!(
        "{} {} {} {}?\n{}",
        gettext("Drop"),
        structure_kind_label(target.kind),
        target.name,
        gettext("from table"),
        target.table.qualified_name()
    )
}

fn structure_action_success_message(request: &StructureActionRequest) -> String {
    match request {
        StructureActionRequest::Rename { target, .. } => {
            format!(
                "{} {}",
                structure_kind_label(target.kind),
                gettext("renamed.")
            )
        }
        StructureActionRequest::Drop { target, mode, .. } => {
            if *mode == StructureDropMode::Cascade {
                return format!(
                    "{} {}",
                    structure_kind_label(target.kind),
                    gettext("dropped with CASCADE.")
                );
            }

            format!(
                "{} {}",
                structure_kind_label(target.kind),
                gettext("dropped.")
            )
        }
    }
}

fn structure_action_failure_message(request: &StructureActionRequest) -> String {
    match request {
        StructureActionRequest::Rename { .. } => gettext("Renaming failed"),
        StructureActionRequest::Drop { mode, .. } => {
            if *mode == StructureDropMode::Cascade {
                gettext("Dropping with CASCADE failed")
            } else {
                gettext("Dropping failed")
            }
        }
    }
}

fn structure_kind_label(kind: StructureActionKind) -> String {
    match kind {
        StructureActionKind::Column => gettext("Column"),
        StructureActionKind::Index => gettext("Index"),
        StructureActionKind::Constraint => gettext("Constraint"),
        StructureActionKind::ForeignKey => gettext("Foreign key"),
        StructureActionKind::Trigger => gettext("Trigger"),
    }
}

fn show_structure_action_error_dialog(
    widgets: &WindowContentWidgets,
    heading: &str,
    error: &str,
    cascade_target: Option<CascadeDropRecovery>,
    sender: &ComponentSender<WindowContent>,
) {
    let body = structure_action_error_body(error, cascade_target.as_ref());
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(&body)
        .close_response("close")
        .build();

    dialog.add_response("close", &gettext("Close"));

    if cascade_target.is_some() {
        dialog.add_response("cascade", &gettext("Drop with CASCADE"));
        dialog.set_response_appearance("cascade", adw::ResponseAppearance::Destructive);
    }

    let root = widgets.toast_overlay.clone();
    let sender = sender.clone();

    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "cascade"
                && let Some(recovery) = cascade_target
            {
                sender.input(WindowContentMsg::StructureActionConfirmed(
                    StructureActionRequest::Drop {
                        scope: recovery.scope,
                        pool: recovery.pool,
                        target: recovery.target,
                        mode: StructureDropMode::Cascade,
                    },
                ));
            }
        },
    );
}

fn structure_action_error_body(
    error: &str,
    cascade_target: Option<&CascadeDropRecovery>,
) -> String {
    if cascade_target.is_none() {
        return error.to_string();
    }

    format!(
        "{}\n\n{}",
        error,
        gettext(
            "This may also drop dependent views, constraints, indexes, or other database objects. This cannot be undone."
        )
    )
}
