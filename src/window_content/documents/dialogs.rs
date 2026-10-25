use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::{self, gio, glib};
use relm4::prelude::*;

use super::{DocumentMsg, SaveChooserIdentity, WindowContent, WindowContentMsg};
use crate::sql_file::{FileError, LoadedSqlFile};

impl WindowContent {
    pub(in crate::window_content) fn choose_sql_files(
        &mut self,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if self.file_operation_pending() || self.active_database_switch_request_id.is_some() {
            return;
        }

        self.documents.chooser_open = true;
        let request = self.document_request();
        self.documents.chooser_request = Some(request);

        let parent = root.root().and_downcast::<gtk::Window>();
        let chooser = gtk::FileChooserNative::new(
            Some(&gettext("Open SQL Files")),
            parent.as_ref(),
            gtk::FileChooserAction::Open,
            Some(&gettext("Open")),
            Some(&gettext("Cancel")),
        );

        chooser.set_select_multiple(true);
        chooser.set_modal(true);
        add_sql_filter(&chooser);

        let sender = sender.input_sender().clone();
        chooser.connect_response(move |chooser, response| {
            let files = if response == gtk::ResponseType::Accept {
                let model = chooser.files();
                (0..model.n_items())
                    .filter_map(|index| model.item(index).and_downcast::<gio::File>())
                    .collect()
            } else {
                Vec::new()
            };

            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::FilesChosen {
                request,
                files,
            }));
            chooser.destroy();
        });

        self.documents.chooser = Some(chooser.clone());
        chooser.show();
    }

    pub(in crate::window_content) fn cancel_document_chooser(&mut self) {
        if let Some(chooser) = self.documents.chooser.take() {
            chooser.destroy();
        }

        self.documents.chooser_open = false;
        self.documents.chooser_request = None;
        self.documents.chooser_identity = None;
    }

    pub(super) fn choose_sql_save_target(
        &mut self,
        id: u64,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let tab = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == id)
            .expect("save tab");

        self.documents.chooser_open = true;
        let parent = root.root().and_downcast::<gtk::Window>();
        let chooser = gtk::FileChooserNative::new(
            Some(&gettext("Save SQL File")),
            parent.as_ref(),
            gtk::FileChooserAction::Save,
            Some(&gettext("Save")),
            Some(&gettext("Cancel")),
        );

        let filename = tab
            .document
            .as_ref()
            .and_then(|doc| doc.file.as_ref())
            .and_then(|file| file.basename())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "query.sql".into());

        chooser.set_current_name(&filename);
        chooser.set_modal(true);
        add_sql_filter(&chooser);

        let request = self.document_request();
        self.documents.chooser_request = Some(request);
        self.documents.chooser_identity =
            self.query_tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| SaveChooserIdentity {
                    tab: id,
                    document_id: tab.document.as_ref().map(|doc| doc.id.clone()),
                    scope: self.active_database_scope().unwrap_or_else(|| {
                        tab.document
                            .as_ref()
                            .expect("document scope")
                            .origin
                            .clone()
                    }),
                });

        let sender = sender.input_sender().clone();
        chooser.connect_response(move |chooser, response| {
            let file = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file())
                .flatten();

            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::SaveChosen {
                tab: id,
                request,
                file,
            }));
            chooser.destroy();
        });

        self.documents.chooser = Some(chooser.clone());
        chooser.show();
    }
}

fn add_sql_filter(chooser: &gtk::FileChooserNative) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(&gettext("SQL Files")));
    filter.add_pattern("*.sql");
    filter.add_pattern("*.SQL");
    chooser.add_filter(&filter);

    let all = gtk::FileFilter::new();
    all.set_name(Some(&gettext("All Files")));
    all.add_pattern("*");
    chooser.add_filter(&all);
}

pub(in crate::window_content) fn present_document_dialog(
    dialog: &adw::AlertDialog,
    root: &adw::ToolbarView,
) {
    let parent = root.root().and_downcast::<gtk::Window>();
    dialog.present(parent.as_ref());
}

pub(in crate::window_content) fn document_error(
    root: &adw::ToolbarView,
    heading: &str,
    body: &str,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();

    dialog.add_response("close", &gettext("Close"));
    dialog.set_close_response("close");
    present_document_dialog(&dialog, root);
}

pub(super) fn show_recovery_warnings(root: &adw::ToolbarView, warnings: &[String]) {
    for warning in warnings {
        glib::g_warning!("codd", "{warning}");
    }
    document_error(
        root,
        &gettext("Some SQL Recovery Files Could Not Be Restored"),
        &recovery_warning_body(warnings),
    );
}

fn recovery_warning_body(warnings: &[String]) -> String {
    let count = u32::try_from(warnings.len()).unwrap_or(u32::MAX);
    let summary = ngettext(
        "One recovery file could not be restored. Valid recovery files were processed normally, and the unreadable file was left untouched.",
        "{count} recovery files could not be restored. Valid recovery files were processed normally, and the unreadable files were left untouched.",
        count,
    ).replace("{count}", &count.to_string());
    let mut details = warnings
        .iter()
        .take(8)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if warnings.len() > 8 {
        details.push_str(&format!(
            "\n{}",
            gettext("See the application logs for the remaining recovery warnings.")
        ));
    }
    format!("{summary}\n\n{details}")
}

pub(super) fn confirm_document_reload(
    id: u64,
    document_id: String,
    revision: u64,
    uri: String,
    root: &adw::ToolbarView,
    sender: &ComponentSender<WindowContent>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Reload SQL File?"))
        .body(gettext(
            "Reloading discards your unsaved changes and cannot be undone.",
        ))
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("reload", &gettext("Reload")),
    ]);
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("reload", adw::ResponseAppearance::Destructive);

    let sender = sender.input_sender().clone();
    dialog.connect_response(Some("reload"), move |_, _| {
        let _ = sender.send(WindowContentMsg::Document(DocumentMsg::ReloadConfirmed {
            tab: id,
            document_id: document_id.clone(),
            revision,
            uri: uri.clone(),
        }));
    });

    present_document_dialog(&dialog, root);
}

pub(super) fn show_document_open_failure(
    id: u64,
    reload: bool,
    file: Option<gio::File>,
    error: FileError,
    root: &adw::ToolbarView,
    sender: &ComponentSender<WindowContent>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Opening SQL File Failed"))
        .body(error.to_string())
        .build();
    dialog.add_responses(&[("cancel", &gettext("Cancel")), ("retry", &gettext("Retry"))]);
    dialog.set_close_response("cancel");

    let sender = sender.input_sender().clone();
    dialog.connect_response(Some("retry"), move |_, _| {
        if reload {
            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::ReloadTab(Some(
                format!("query-tab-{id}"),
            ))));
        } else {
            let _ = sender.send(WindowContentMsg::OpenSqlFileLocations(vec![
                file.clone().expect("loaded file"),
            ]));
        }
    });

    present_document_dialog(&dialog, root);
}

pub(super) fn show_document_save_failure(
    id: u64,
    error: FileError,
    root: &adw::ToolbarView,
    sender: &ComponentSender<WindowContent>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Saving SQL File Failed"))
        .body(format!(
            "{error}\n{}",
            gettext("Check the file permissions or use Save As to choose another location.")
        ))
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("save-as", &gettext("Save As")),
    ]);
    dialog.set_close_response("cancel");

    let sender = sender.input_sender().clone();
    dialog.connect_response(Some("save-as"), move |_, _| {
        let _ = sender.send(WindowContentMsg::Document(DocumentMsg::SaveTab(
            Some(format!("query-tab-{id}")),
            true,
        )));
    });

    present_document_dialog(&dialog, root);
}

pub(super) fn show_document_save_conflict(
    request: u64,
    result: Result<LoadedSqlFile, FileError>,
    root: &adw::ToolbarView,
    sender: &ComponentSender<WindowContent>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("File Changed on Disk"))
        .body(gettext("The file was changed, deleted, or cannot be verified. Save a copy, or explicitly replace the latest verified version."))
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("save-as", &gettext("Save As")),
    ]);
    let deleted = matches!(result, Err(FileError::Deleted));
    let etag = result.ok().and_then(|loaded| loaded.etag);
    if etag.is_some() {
        dialog.add_response("overwrite", &gettext("Overwrite"));
        dialog.set_response_appearance("overwrite", adw::ResponseAppearance::Destructive);
    }
    if deleted {
        dialog.add_response("recreate", &gettext("Recreate File"));
        dialog.set_response_appearance("recreate", adw::ResponseAppearance::Destructive);
    }
    dialog.set_close_response("cancel");

    let sender = sender.input_sender().clone();
    dialog.connect_response(None, move |_, response| {
        if response == "overwrite" || response == "recreate" {
            let _ = sender.send(WindowContentMsg::Document(
                DocumentMsg::OverwriteConfirmed {
                    request,
                    etag: etag.clone(),
                },
            ));
        } else if response == "save-as" {
            let _ = sender.send(WindowContentMsg::Document(
                DocumentMsg::SaveAsRequestedAfterConflict(request),
            ));
        } else {
            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::SaveCancelled(
                request,
            )));
        }
    });

    present_document_dialog(&dialog, root);
}

pub(super) fn show_promotion_recovery_failure(
    id: u64,
    document_id: String,
    error: String,
    root: &adw::ToolbarView,
    sender: &ComponentSender<WindowContent>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Saving SQL Recovery Failed"))
        .body(format!(
            "{error}\n{}",
            gettext("The file was saved, but newer edits cannot be recovered after a crash. Retry before removing the scratch-session backup.")
        ))
        .build();
    dialog.add_responses(&[
        ("continue", &gettext("Keep Editing Without Recovery")),
        ("retry", &gettext("Retry")),
    ]);
    dialog.set_default_response(Some("retry"));
    dialog.set_close_response("continue");

    let sender = sender.input_sender().clone();
    dialog.connect_response(None, move |_, response| {
        let msg = if response == "retry" {
            DocumentMsg::RetryPromotionRecovery {
                tab: id,
                document_id: document_id.clone(),
            }
        } else {
            DocumentMsg::ContinueWithoutRecovery {
                tab: id,
                document_id: document_id.clone(),
            }
        };
        let _ = sender.send(WindowContentMsg::Document(msg));
    });

    present_document_dialog(&dialog, root);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_warnings_include_the_count_and_file_details() {
        let warnings = vec![
            "broken-a.json: invalid JSON".into(),
            "broken-b.json: unreadable".into(),
        ];
        let body = recovery_warning_body(&warnings);
        assert!(body.contains("2 recovery files"));
        assert!(body.contains("broken-a.json"));
        assert!(body.contains("broken-b.json"));
    }

    #[test]
    fn many_recovery_warnings_keep_an_exact_count_without_an_unbounded_dialog_body() {
        let warnings = (0..12)
            .map(|index| format!("broken-{index}.json: invalid JSON"))
            .collect::<Vec<_>>();
        let body = recovery_warning_body(&warnings);
        assert!(body.contains("12 recovery files"));
        assert!(body.contains("broken-0.json"));
        assert!(body.contains("broken-7.json"));
        assert!(!body.contains("broken-8.json"));
        assert!(body.contains("application logs"));
    }
}
