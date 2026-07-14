use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::db;
use crate::db::object_actions::TruncateOptions;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::ui::components::sidebar::ObjectAction;

use super::{WindowContent, WindowContentCommandOutput, WindowContentMsg, WindowContentWidgets};

#[derive(Debug)]
pub(crate) enum ObjectActionRequest {
    Rename {
        object: DatabaseObject,
        new_name: String,
    },
    Truncate {
        object: DatabaseObject,
        options: TruncateOptions,
    },
    Delete {
        object: DatabaseObject,
    },
}

impl ObjectActionRequest {
    fn action(&self) -> ObjectAction {
        match self {
            Self::Rename { .. } => ObjectAction::Rename,
            Self::Truncate { .. } => ObjectAction::Truncate,
            Self::Delete { .. } => ObjectAction::Delete,
        }
    }

    fn object(&self) -> &DatabaseObject {
        match self {
            Self::Rename { object, .. }
            | Self::Truncate { object, .. }
            | Self::Delete { object } => object,
        }
    }
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
                    ObjectActionRequest::Rename {
                        object,
                        new_name: entry.text().to_string(),
                    },
                ));
            }
        },
    );
}

pub(super) fn show_delete_object_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<WindowContent>,
    object: DatabaseObject,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(object_action_heading(ObjectAction::Delete, &object))
        .body(object_action_body(ObjectAction::Delete, &object))
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        (
            "confirm",
            &object_action_confirm_label(ObjectAction::Delete),
        ),
    ]);

    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);

    let sender = sender.clone();

    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "confirm" {
                sender.input(WindowContentMsg::ObjectActionConfirmed(
                    ObjectActionRequest::Delete { object },
                ));
            }
        },
    );
}

pub(super) fn show_truncate_object_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<WindowContent>,
    object: DatabaseObject,
) {
    let restart_identity_row = adw::SwitchRow::builder()
        .title(gettext("Restart identity"))
        .subtitle(gettext(
            "Reset sequences owned by columns of the truncated table.",
        ))
        .build();

    let cascade_row = adw::SwitchRow::builder()
        .title(gettext("Cascade"))
        .subtitle(gettext(
            "Also truncate tables that have foreign-key references to this table.",
        ))
        .build();

    let options_group = adw::PreferencesGroup::builder().margin_top(12).build();

    options_group.add(&restart_identity_row);
    options_group.add(&cascade_row);

    let dialog = adw::AlertDialog::builder()
        .heading(object_action_heading(ObjectAction::Truncate, &object))
        .body(object_action_body(ObjectAction::Truncate, &object))
        .extra_child(&options_group)
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        (
            "confirm",
            &object_action_confirm_label(ObjectAction::Truncate),
        ),
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

            let truncate_options = TruncateOptions {
                restart_identity: restart_identity_row.is_active(),
                cascade: cascade_row.is_active(),
            };

            sender.input(WindowContentMsg::ObjectActionConfirmed(
                ObjectActionRequest::Truncate {
                    object,
                    options: truncate_options,
                },
            ));
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
        let action = request.action();
        let object = request.object().clone();

        match result {
            Ok(()) => {
                match request {
                    ObjectActionRequest::Rename { new_name, .. } => {
                        self.apply_renamed_object(&object, &new_name, widgets);
                    }
                    ObjectActionRequest::Truncate { .. } => {
                        self.reload_browse_tab(&object);
                    }
                    ObjectActionRequest::Delete { .. } => {
                        self.remove_deleted_object(&object, widgets);
                    }
                }

                self.reload_schema(sender);
                self.broadcast_schema_changed();

                widgets
                    .toast_overlay
                    .add_toast(adw::Toast::new(&object_action_success_message(
                        action, &object,
                    )));
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    object_action_failure_message(action)
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
                show_truncate_object_dialog(&widgets.toast_overlay, sender, object);
            }
            ObjectAction::Delete => {
                show_delete_object_dialog(&widgets.toast_overlay, sender, object);
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
            let result = match &request {
                ObjectActionRequest::Rename { object, new_name } => {
                    db::object_actions::rename_object(&pool, object, new_name)
                        .await
                        .map_err(|error| error.to_string())
                }
                ObjectActionRequest::Truncate { object, options } => {
                    db::object_actions::truncate_table(&pool, object, *options)
                        .await
                        .map_err(|error| error.to_string())
                }
                ObjectActionRequest::Delete { object } => {
                    db::object_actions::drop_object(&pool, object)
                        .await
                        .map_err(|error| error.to_string())
                }
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
        ObjectAction::Truncate => gettext("Remove all rows from {table}?\nThis cannot be undone.")
            .replace("{table}", &object.qualified_name()),
        ObjectAction::Delete => {
            let message = match object.kind {
                DatabaseObjectKind::Table => {
                    gettext("Delete table {name}?\nThis cannot be undone.")
                }
                DatabaseObjectKind::View => gettext("Delete view {name}?\nThis cannot be undone."),
            };

            message.replace("{name}", &object.qualified_name())
        }
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
        ObjectAction::Truncate => {
            gettext("Table truncated: {table}").replace("{table}", &object.qualified_name())
        }
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
    request: ObjectActionRequest,
    widgets: &WindowContentWidgets,
) -> Option<ObjectActionRequest> {
    match request {
        ObjectActionRequest::Rename { object, new_name } => {
            let new_name = match db::object_actions::normalize_new_object_name(&new_name) {
                Some(name) => name,
                None => {
                    widgets
                        .toast_overlay
                        .add_toast(adw::Toast::new(&gettext("Enter a valid object name.")));
                    return None;
                }
            };

            if new_name == object.name {
                return None;
            }

            Some(ObjectActionRequest::Rename { object, new_name })
        }
        ObjectActionRequest::Truncate { .. } | ObjectActionRequest::Delete { .. } => Some(request),
    }
}
