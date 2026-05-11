use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::ui::components::sidebar::ObjectAction;

use super::{WindowContent, WindowContentCommandOutput, WindowContentMsg, WindowContentWidgets};

#[derive(Debug)]
pub(crate) struct ObjectActionRequest {
    action: ObjectAction,
    object: DatabaseObject,
    new_name: Option<String>,
}

pub(super) fn show_rename_object_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<WindowContent>,
    object: DatabaseObject,
) {
    let entry = gtk::Entry::builder()
        .text(&object.name)
        .activates_default(true)
        .hexpand(true)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading(object_action_heading(ObjectAction::Rename, &object))
        .body(gettext("Choose a new name for this database object."))
        .extra_child(&entry)
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("rename", &gettext("Rename")),
    ]);

    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
    dialog.set_response_enabled(
        "rename",
        db::object_actions::normalize_new_object_name(&object.name)
            .is_some_and(|name| name != object.name),
    );

    entry.connect_changed({
        let dialog = dialog.clone();
        let current_name = object.name.clone();

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
                sender.input(WindowContentMsg::ObjectActionConfirmed(
                    ObjectActionRequest {
                        action: ObjectAction::Rename,
                        object,
                        new_name: Some(entry.text().to_string()),
                    },
                ));
            }
        },
    );
}

pub(super) fn show_confirm_object_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<WindowContent>,
    object: DatabaseObject,
    action: ObjectAction,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(object_action_heading(action, &object))
        .body(object_action_body(action, &object))
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("confirm", &object_action_confirm_label(action)),
    ]);

    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);

    let sender = sender.clone();

    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response != "confirm" {
                return;
            }

            match action {
                ObjectAction::Rename => {}
                ObjectAction::Truncate => {
                    sender.input(WindowContentMsg::ObjectActionConfirmed(
                        ObjectActionRequest {
                            action,
                            object,
                            new_name: None,
                        },
                    ));
                }
                ObjectAction::Delete => {
                    sender.input(WindowContentMsg::ObjectActionConfirmed(
                        ObjectActionRequest {
                            action,
                            object,
                            new_name: None,
                        },
                    ));
                }
            }
        },
    );
}

impl WindowContent {
    pub(super) fn handle_object_action_completed(
        &mut self,
        request: ObjectActionRequest,
        result: Result<(), String>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => {
                match request.action {
                    ObjectAction::Rename => {
                        if let Some(new_name) = request.new_name {
                            self.apply_renamed_object(&request.object, &new_name, widgets);
                        }
                    }
                    ObjectAction::Truncate => {
                        self.reload_browse_tab(&request.object);
                    }
                    ObjectAction::Delete => {
                        self.remove_deleted_object(&request.object, widgets);
                    }
                }

                self.reload_schema(sender);
                widgets
                    .toast_overlay
                    .add_toast(adw::Toast::new(&object_action_success_message(
                        request.action,
                        &request.object,
                    )));
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    object_action_failure_message(request.action)
                )));
            }
        }
    }

    pub(super) fn handle_object_action(
        &self,
        object: DatabaseObject,
        action: ObjectAction,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        match action {
            ObjectAction::Rename => {
                show_rename_object_dialog(&widgets.toast_overlay, sender, object)
            }
            ObjectAction::Truncate => {
                show_confirm_object_dialog(&widgets.toast_overlay, sender, object, action);
            }
            ObjectAction::Delete => {
                show_confirm_object_dialog(&widgets.toast_overlay, sender, object, action);
            }
        }
    }

    pub(super) fn run_object_action(
        &self,
        request: ObjectActionRequest,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pool) = self.active_pool.clone() else {
            return;
        };

        let request = match normalize_object_action_request(request, widgets) {
            Some(request) => request,
            None => return,
        };

        sender.oneshot_command(async move {
            let result = match request.action {
                ObjectAction::Rename => {
                    if let Some(new_name) = request.new_name.as_deref() {
                        db::object_actions::rename_object(&pool, &request.object, new_name)
                            .await
                            .map_err(|error| error.to_string())
                    } else {
                        Err(gettext("Missing new object name."))
                    }
                }
                ObjectAction::Truncate => {
                    db::object_actions::truncate_table(&pool, &request.object)
                        .await
                        .map_err(|error| error.to_string())
                }
                ObjectAction::Delete => db::object_actions::drop_object(&pool, &request.object)
                    .await
                    .map_err(|error| error.to_string()),
            };

            WindowContentCommandOutput::ObjectActionFinished(request, result)
        });
    }
}

fn object_action_heading(action: ObjectAction, object: &DatabaseObject) -> String {
    match action {
        ObjectAction::Rename => match object.kind {
            DatabaseObjectKind::Table => gettext("Rename Table"),
            DatabaseObjectKind::View => gettext("Rename View"),
        },
        ObjectAction::Truncate => gettext("Truncate Table"),
        ObjectAction::Delete => match object.kind {
            DatabaseObjectKind::Table => gettext("Delete Table"),
            DatabaseObjectKind::View => gettext("Delete View"),
        },
    }
}

fn object_action_body(action: ObjectAction, object: &DatabaseObject) -> String {
    match action {
        ObjectAction::Rename => String::new(),
        ObjectAction::Truncate => format!(
            "{} {}?\n{}",
            gettext("Remove all rows from"),
            object.qualified_name(),
            gettext("This cannot be undone.")
        ),
        ObjectAction::Delete => format!(
            "{} {}?\n{}",
            gettext("Delete"),
            object.qualified_name(),
            gettext("This cannot be undone.")
        ),
    }
}

fn object_action_confirm_label(action: ObjectAction) -> String {
    match action {
        ObjectAction::Rename => gettext("Rename"),
        ObjectAction::Truncate => gettext("Truncate"),
        ObjectAction::Delete => gettext("Delete"),
    }
}

fn object_action_success_message(action: ObjectAction, object: &DatabaseObject) -> String {
    match action {
        ObjectAction::Rename => gettext("Database object renamed."),
        ObjectAction::Truncate => format!(
            "{} {}",
            gettext("Table truncated:"),
            object.qualified_name()
        ),
        ObjectAction::Delete => gettext("Database object deleted."),
    }
}

fn object_action_failure_message(action: ObjectAction) -> String {
    match action {
        ObjectAction::Rename => gettext("Renaming failed"),
        ObjectAction::Truncate => gettext("Truncating failed"),
        ObjectAction::Delete => gettext("Deleting failed"),
    }
}

fn normalize_object_action_request(
    mut request: ObjectActionRequest,
    widgets: &WindowContentWidgets,
) -> Option<ObjectActionRequest> {
    match request.action {
        ObjectAction::Rename => {
            let new_name = match db::object_actions::normalize_new_object_name(
                request.new_name.as_deref().unwrap_or_default(),
            ) {
                Some(name) => name,
                None => {
                    widgets
                        .toast_overlay
                        .add_toast(adw::Toast::new(&gettext("Enter a valid object name.")));
                    return None;
                }
            };

            if new_name == request.object.name {
                return None;
            }

            request.new_name = Some(new_name);
        }
        ObjectAction::Truncate | ObjectAction::Delete => {
            request.new_name = None;
        }
    }

    Some(request)
}
