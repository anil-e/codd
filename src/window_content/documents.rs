use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::{self, gio, glib};
use relm4::prelude::*;
use uuid::Uuid;

use super::{
    SqlEditorMsg, WindowContent, WindowContentMsg, WindowContentWidgets, tabs,
    with_window_content_subscribers,
};
use crate::models::session::SavedSession;
use crate::sql_file::{self, FileError, LoadedSqlFile, SaveTarget};
use crate::state::document_store::{RecoveryLoad, RecoveryRecord};

mod dialogs;
mod recovery;

use dialogs::{
    confirm_document_reload, show_document_open_failure, show_document_save_conflict,
    show_document_save_failure, show_promotion_recovery_failure, show_recovery_warnings,
};
pub(super) use dialogs::{document_error, present_document_dialog};
use recovery::{promotion_sources, suppress_promoted_scratch_tabs};

thread_local! {
    static OWNERS: RefCell<HashMap<String, (u64, u64)>> = RefCell::new(HashMap::new());
}

pub(super) struct SqlDocument {
    pub file: Option<gio::File>,
    pub saved_text: String,
    pub external_changed: bool,
    recovered_from: Option<String>,
    id: String,
    origin: (String, String),
    etag: Option<String>,
    bom: bool,
    revision: u64,
    request: Option<u64>,
    monitor: Option<gio::FileMonitor>,
    monitor_generation: u64,
    disk_check_request: Option<u64>,
    promotion_pending: bool,
    source_tab_id: Option<u64>,
}

impl SqlDocument {
    fn is_busy(&self) -> bool {
        self.request.is_some() || self.promotion_pending
    }

    fn matches_monitor(&self, id: &str, uri: &str, generation: u64) -> bool {
        self.id == id
            && self.monitor_generation == generation
            && self.file.as_ref().is_some_and(|file| file.uri() == uri)
    }

    pub fn is_dirty(&self, text: &str) -> bool {
        self.file.is_none() || self.saved_text != text
    }

    fn recovery_record(&self, text: String) -> RecoveryRecord {
        RecoveryRecord {
            document_id: self.id.clone(),
            connection_id: self.origin.0.clone(),
            database: self.origin.1.clone(),
            uri: self.file.as_ref().map(|file| file.uri().to_string()),
            text,
            saved_text: self.saved_text.clone(),
            etag: self.etag.clone(),
            bom: self.bom,
            source_tab_id: self.source_tab_id,
        }
    }

    pub fn title(&self) -> String {
        self.file
            .as_ref()
            .and_then(|file| file.basename())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| gettext("Recovered Script (Save As Required)"))
    }

    pub fn tooltip(&self) -> String {
        let location = self
            .file
            .as_ref()
            .map(|file| file.uri().to_string())
            .or_else(|| self.recovered_from.clone())
            .unwrap_or_default();
        if self.external_changed {
            format!(
                "{location}\n{}",
                gettext("File changed on disk. Reload to read the latest version.")
            )
        } else {
            location
        }
    }

    fn validate_recovery(
        &mut self,
        current: &str,
        result: Result<LoadedSqlFile, FileError>,
    ) -> Option<String> {
        match result {
            Ok(loaded) if !self.is_dirty(current) => {
                self.saved_text = loaded.text.clone();
                self.etag = loaded.etag;
                self.bom = loaded.bom;
                self.external_changed = false;
                Some(loaded.text)
            }
            Ok(loaded) => {
                self.external_changed = loaded.text != self.saved_text || loaded.bom != self.bom;
                if !self.external_changed {
                    self.etag = loaded.etag;
                }
                None
            }
            Err(_) => {
                self.external_changed = true;
                if let Some(file) = self.file.take() {
                    self.recovered_from = Some(file.uri().to_string());
                }
                None
            }
        }
    }
}

impl Drop for SqlDocument {
    fn drop(&mut self) {
        if let Some(monitor) = &self.monitor {
            monitor.cancel();
        }
    }
}

#[derive(Default)]
pub(super) struct DocumentWorkspace {
    pub queued_files: Vec<gio::File>,
    restored: HashSet<(String, String)>,
    next_request: u64,
    saves: HashMap<u64, SaveSnapshot>,
    chooser_open: bool,
    chooser: Option<gtk::FileChooserNative>,
    chooser_request: Option<u64>,
    recovery_pending: usize,
    recovery_errors_shown: HashSet<String>,
    chooser_identity: Option<SaveChooserIdentity>,
    pending_session: Option<PendingSession>,
}

struct PendingSession {
    scope: (String, String),
    session: Option<SavedSession>,
}

impl DocumentWorkspace {
    fn has_pending_operations(&self, pending_reads: bool) -> bool {
        self.chooser_open || !self.saves.is_empty() || self.recovery_pending != 0 || pending_reads
    }
}

struct SaveChooserIdentity {
    tab: u64,
    document_id: Option<String>,
    scope: (String, String),
}

impl SaveChooserIdentity {
    fn matches(
        &self,
        tab: u64,
        document_id: Option<&str>,
        scope: Option<&(String, String)>,
    ) -> bool {
        self.tab == tab && self.document_id.as_deref() == document_id && Some(&self.scope) == scope
    }
}

struct SaveSnapshot {
    tab: u64,
    document_id: Option<String>,
    file: gio::File,
    text: String,
    bom: bool,
    origin: (String, String),
    close_generation: Option<u64>,
    source_tab_id: Option<u64>,
}

#[derive(Debug)]
pub enum DocumentMsg {
    FilesChosen {
        request: u64,
        files: Vec<gio::File>,
    },
    SaveChosen {
        tab: u64,
        request: u64,
        file: Option<gio::File>,
    },
    Loaded {
        tab: u64,
        document_id: String,
        request: u64,
        reload: bool,
        result: Result<LoadedSqlFile, FileError>,
    },
    Saved {
        request: u64,
        result: Result<Option<String>, FileError>,
    },
    SaveTab(Option<String>, bool),
    ReloadTab(Option<String>),
    ReloadConfirmed {
        tab: u64,
        document_id: String,
        revision: u64,
        uri: String,
    },
    ExternalChange {
        tab: u64,
        document_id: String,
        uri: String,
        generation: u64,
    },
    DiskChecked {
        tab: u64,
        document_id: String,
        uri: String,
        generation: u64,
        request: u64,
        baseline_etag: Option<String>,
        result: Result<Option<String>, String>,
    },
    RecoveryValidated {
        tab: u64,
        document_id: String,
        request: u64,
        result: Result<LoadedSqlFile, FileError>,
    },
    SyncFileActions,
    FlushRecovery {
        tab: u64,
        document_id: String,
        revision: u64,
    },
    RecoveryLoaded {
        scope: (String, String),
        result: Result<RecoveryLoad, String>,
    },
    RecoveryStored {
        document_id: String,
        result: Result<(), String>,
    },
    RetryPromotionRecovery {
        tab: u64,
        document_id: String,
    },
    ContinueWithoutRecovery {
        tab: u64,
        document_id: String,
    },
    OverwriteVersion {
        request: u64,
        result: Result<LoadedSqlFile, FileError>,
    },
    OverwriteConfirmed {
        request: u64,
        etag: Option<String>,
    },
    SaveAsRequestedAfterConflict(u64),
    SaveCancelled(u64),
}

impl WindowContent {
    pub(super) fn document_session_restore_pending(&self) -> bool {
        self.documents.pending_session.is_some()
    }
    pub(super) fn has_pending_document_promotion(&self) -> bool {
        self.query_tabs.iter().any(|tab| {
            tab.document
                .as_ref()
                .is_some_and(|doc| doc.promotion_pending)
        })
    }
    pub(super) fn file_operation_pending(&self) -> bool {
        self.documents.has_pending_operations(
            self.query_tabs
                .iter()
                .any(|tab| tab.document.as_ref().is_some_and(SqlDocument::is_busy)),
        )
    }

    pub(super) fn sync_file_actions(
        &self,
        root: &adw::ToolbarView,
        widgets: &WindowContentWidgets,
    ) {
        let Some(window) = root.root().and_downcast::<adw::ApplicationWindow>() else {
            return;
        };
        let selected = tabs::selected_query_tab_id(widgets)
            .and_then(|id| self.query_tabs.iter().find(|tab| tab.id == id));
        let can_save_as = self.shows_workspace()
            && selected.is_some()
            && !self.file_operation_pending()
            && !self.close_preparation_pending()
            && self.active_database_switch_request_id.is_none();
        let can_save = can_save_as
            && selected.is_some_and(|tab| {
                tab.document.as_ref().is_none_or(|doc| {
                    doc.is_dirty(&self.query_tab_sql(tab.id).unwrap_or_default())
                        || doc.external_changed
                })
            });
        crate::window_actions::set_file_actions_enabled(&window, can_save, can_save_as);
    }

    pub(super) fn document_input_blocked(&self, msg: &WindowContentMsg) -> bool {
        (self.close_preparation_pending() || self.documents.pending_session.is_some())
            && !matches!(
                msg,
                WindowContentMsg::Document(_)
                    | WindowContentMsg::Closing(_)
                    | WindowContentMsg::CancelClosePreparation
                    | WindowContentMsg::CommitClose
                    | WindowContentMsg::PrepareClose
                    | WindowContentMsg::SaveSession
                    | WindowContentMsg::QueryTabTitleChanged(_)
                    | WindowContentMsg::BrowseTabOutput {
                        output: super::TableViewOutput::EditsClosePrepared(_)
                            | super::TableViewOutput::EditsCloseCancelled(_)
                            | super::TableViewOutput::EditsCloseCommitted(_)
                            | super::TableViewOutput::PendingEditsChanged(_),
                        ..
                    }
                    | WindowContentMsg::WindowEvent(super::WindowContentEvent::Connections(_))
                    | WindowContentMsg::DatabaseSwitchCompleted { .. }
                    | WindowContentMsg::ConnectionDialogOutput(_)
            )
    }

    fn document_request(&mut self) -> u64 {
        self.documents.next_request += 1;
        self.documents.next_request
    }

    fn reserve_document(&self, file: &gio::File, tab: u64) -> bool {
        self.claim_document(file.uri().to_string(), tab)
    }

    fn claim_document(&self, uri: String, tab: u64) -> bool {
        OWNERS.with(|owners| {
            let mut owners = owners.borrow_mut();
            if let Some(&(owner, existing)) = owners.get(&uri) {
                if (owner, existing) == (self.sync_subscription_id, tab) {
                    return true;
                }
                with_window_content_subscribers(|subscribers| {
                    if let Some(sender) = subscribers.get(&owner) {
                        let _ = sender.send(WindowContentMsg::FocusSqlDocument(existing));
                    }
                });
                return false;
            }
            owners.insert(uri, (self.sync_subscription_id, tab));
            true
        })
    }

    pub(super) fn release_document_tab(&self, tab: u64) {
        OWNERS.with(|owners| {
            owners
                .borrow_mut()
                .retain(|_, owner| *owner != (self.sync_subscription_id, tab))
        });
    }

    fn release_uri(&self, file: &gio::File, tab: u64) {
        OWNERS.with(|owners| {
            let mut owners = owners.borrow_mut();
            if owners.get(file.uri().as_str()) == Some(&(self.sync_subscription_id, tab)) {
                owners.remove(file.uri().as_str());
            }
        });
    }

    pub(super) fn release_document_ownership(&self) {
        OWNERS.with(|owners| {
            owners
                .borrow_mut()
                .retain(|_, (owner, _)| *owner != self.sync_subscription_id)
        });
    }

    pub(super) fn focus_sql_document(
        &mut self,
        id: u64,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
    ) {
        if let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == id) {
            widgets.query_tab_view.set_selected_page(&tab.page);
            self.active_query_tab_id = id;
            if let Some(window) = root.root().and_downcast::<gtk::Window>() {
                window.present();
            }
            tab.editor.emit(SqlEditorMsg::Focus);
        }
    }

    pub(super) fn open_sql_locations(
        &mut self,
        files: Vec<gio::File>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if !self.shows_workspace() {
            self.documents.queued_files.extend(files);
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Connect to PostgreSQL to open the queued SQL files.",
            )));
            return;
        }
        if self.file_operation_pending() || self.active_database_switch_request_id.is_some() {
            self.documents.queued_files.extend(files);
            return;
        }
        for file in files {
            let id = self.next_query_tab_id;
            if !self.reserve_document(&file, id) {
                continue;
            }
            self.add_query_tab(widgets, sender);
            let request = self.document_request();
            let document = SqlDocument {
                file: Some(file.clone()),
                external_changed: false,
                recovered_from: None,
                monitor_generation: 0,
                disk_check_request: None,
                promotion_pending: false,
                source_tab_id: None,
                saved_text: String::new(),
                id: Uuid::new_v4().to_string(),
                origin: self.active_database_scope().expect("connected workspace"),
                etag: None,
                bom: false,
                revision: 0,
                request: Some(request),
                monitor: None,
            };
            let document_id = document.id.clone();
            self.claim_document(format!("document:{document_id}"), id);
            let tab = self.query_tab_mut(id).expect("new query tab");
            tab.document = Some(document);
            tab.page.set_loading(true);
            tab.editor.emit(SqlEditorMsg::SetEditable(false));
            self.update_query_tab_title(id);
            let sender = sender.input_sender().clone();
            glib::spawn_future_local(async move {
                let result = sql_file::load(&file).await;
                let _ = sender.send(WindowContentMsg::Document(DocumentMsg::Loaded {
                    tab: id,
                    document_id,
                    request,
                    reload: false,
                    result,
                }));
            });
        }
    }

    pub(super) fn save_selected_document(
        &mut self,
        save_as: bool,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if !self.shows_workspace() {
            return;
        }
        if let Some(id) = tabs::selected_query_tab_id(widgets) {
            self.save_document(id, save_as, root, sender);
        }
    }

    pub(super) fn save_document(
        &mut self,
        id: u64,
        save_as: bool,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if self.file_operation_pending() || self.active_database_switch_request_id.is_some() {
            return;
        }
        if self.documents.saves.values().any(|save| save.tab == id) {
            return;
        }
        let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        if tab
            .document
            .as_ref()
            .is_some_and(|doc| doc.request.is_some())
        {
            return;
        }
        if save_as || tab.document.as_ref().is_none_or(|doc| doc.file.is_none()) {
            if self.documents.chooser_open {
                return;
            }
            self.choose_sql_save_target(id, root, sender);
        } else if let Some(document) = &tab.document
            && let Some(file) = document.file.clone()
        {
            let target = SaveTarget::Existing {
                etag: document.etag.clone(),
            };
            self.start_document_save(id, file, target, sender);
        }
    }

    fn start_document_save(
        &mut self,
        id: u64,
        file: gio::File,
        target: SaveTarget,
        sender: &ComponentSender<Self>,
    ) {
        let scratch = self
            .query_tabs
            .iter()
            .any(|tab| tab.id == id && tab.document.is_none());
        if self.active_database_switch_request_id.is_some()
            || self.file_operation_pending()
            || self
                .query_tabs
                .iter()
                .find(|tab| tab.id == id)
                .is_none_or(|tab| {
                    tab.document
                        .as_ref()
                        .is_some_and(|doc| doc.request.is_some())
                })
        {
            if scratch {
                self.pending_navigation.disconnect_pending = false;
            }
            return;
        }
        if !self.reserve_document(&file, id) {
            if scratch {
                self.pending_navigation.disconnect_pending = false;
            }
            if self.closing.preparation.is_some() {
                sender.input(WindowContentMsg::CancelClosePreparation);
            }
            return;
        }
        let Some(text) = self.query_tab_sql(id) else {
            self.release_uri(&file, id);
            return;
        };
        let Some(origin) = self
            .query_tabs
            .iter()
            .find(|tab| tab.id == id)
            .and_then(|tab| tab.document.as_ref())
            .map(|doc| doc.origin.clone())
            .or_else(|| self.active_database_scope())
        else {
            self.release_uri(&file, id);
            return;
        };
        let request = self.document_request();
        let close_generation = self
            .closing
            .preparation
            .as_ref()
            .map(|close| close.generation);
        let tab = self.query_tab_mut(id).expect("save tab");
        let document_id = tab.document.as_ref().map(|doc| doc.id.clone());
        let source_tab_id = tab
            .document
            .as_ref()
            .map_or(Some(tab.session_id), |doc| doc.source_tab_id);
        let bom = tab.document.as_ref().is_some_and(|doc| doc.bom);
        tab.page.set_loading(true);
        self.documents.saves.insert(
            request,
            SaveSnapshot {
                tab: id,
                document_id,
                file: file.clone(),
                text: text.clone(),
                bom,
                origin,
                close_generation,
                source_tab_id,
            },
        );
        let sender = sender.input_sender().clone();
        glib::spawn_future_local(async move {
            let result = sql_file::save(&file, text, bom, target).await;
            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::Saved {
                request,
                result,
            }));
        });
    }

    fn attach_document_monitor(&mut self, id: u64, sender: &ComponentSender<Self>) {
        let Some(document) = self.query_tab_mut(id).and_then(|tab| tab.document.as_mut()) else {
            return;
        };
        if let Some(monitor) = document.monitor.take() {
            monitor.cancel();
        }
        document.monitor_generation += 1;
        document.disk_check_request = None;
        let generation = document.monitor_generation;
        let Some(file) = document.file.as_ref() else {
            return;
        };
        let uri = file.uri().to_string();
        if let Ok(monitor) =
            file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            let document_id = document.id.clone();
            let sender = sender.input_sender().clone();
            monitor.connect_changed(move |_, _, _, event| {
                if matches!(
                    event,
                    gio::FileMonitorEvent::ChangesDoneHint
                        | gio::FileMonitorEvent::Deleted
                        | gio::FileMonitorEvent::MovedOut
                        | gio::FileMonitorEvent::Renamed
                ) {
                    let _ = sender.send(WindowContentMsg::Document(DocumentMsg::ExternalChange {
                        tab: id,
                        document_id: document_id.clone(),
                        uri: uri.clone(),
                        generation,
                    }));
                }
            });
            document.monitor = Some(monitor);
        }
    }

    fn reload_document(
        &mut self,
        id: u64,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
        confirmed: bool,
    ) {
        let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let Some(doc) = &tab.document else {
            return;
        };
        let Some(file) = &doc.file else {
            return;
        };
        if doc.request.is_some() || self.documents.saves.values().any(|save| save.tab == id) {
            return;
        }
        if !confirmed && self.query_tab_sql(id).as_deref() != Some(doc.saved_text.as_str()) {
            confirm_document_reload(
                id,
                doc.id.clone(),
                doc.revision,
                file.uri().to_string(),
                root,
                sender,
            );
            return;
        }
        let request = self.document_request();
        let tab = self.query_tab_mut(id).expect("reload tab");
        let doc = tab.document.as_mut().expect("reload document");
        doc.request = Some(request);
        let file = doc.file.clone().expect("reload target");
        let document_id = doc.id.clone();
        tab.page.set_loading(true);
        tab.editor.emit(SqlEditorMsg::SetEditable(false));
        let sender = sender.input_sender().clone();
        glib::spawn_future_local(async move {
            let result = sql_file::load(&file).await;
            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::Loaded {
                tab: id,
                document_id,
                request,
                reload: true,
                result,
            }));
        });
    }

    pub(super) fn handle_document_message(
        &mut self,
        msg: DocumentMsg,
        widgets: &mut WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        match msg {
            DocumentMsg::FilesChosen { request, files } => {
                if self.documents.chooser_request != Some(request) {
                    return;
                }
                self.documents.chooser_request = None;
                self.documents.chooser_open = false;
                self.documents.chooser = None;
                self.open_sql_locations(files, widgets, sender);
            }
            DocumentMsg::SaveChosen { tab, request, file } => {
                if self.documents.chooser_request != Some(request) {
                    return;
                }
                let identity = self.documents.chooser_identity.take();
                let was_scratch = identity
                    .as_ref()
                    .is_some_and(|identity| identity.document_id.is_none());
                self.documents.chooser_request = None;
                self.documents.chooser_open = false;
                self.documents.chooser = None;
                let valid = identity.is_some_and(|identity| {
                    self.query_tabs
                        .iter()
                        .find(|item| item.id == tab)
                        .is_some_and(|item| {
                            identity.matches(
                                tab,
                                item.document.as_ref().map(|doc| doc.id.as_str()),
                                self.active_database_scope()
                                    .or_else(|| {
                                        item.document.as_ref().map(|doc| doc.origin.clone())
                                    })
                                    .as_ref(),
                            )
                        })
                });
                if !valid
                    || self.active_database_switch_request_id.is_some()
                    || self.file_operation_pending()
                    || self
                        .closing
                        .preparation
                        .as_ref()
                        .is_some_and(|close| close.cancelling || close.approved)
                {
                    if was_scratch {
                        self.pending_navigation.disconnect_pending = false;
                    }
                    self.cancel_close(root, sender);
                    return;
                }
                if let Some(file) = file {
                    let target = self
                        .query_tabs
                        .iter()
                        .find(|item| item.id == tab)
                        .and_then(|item| item.document.as_ref())
                        .filter(|doc| doc.file.as_ref() == Some(&file))
                        .map(|doc| SaveTarget::Existing {
                            etag: doc.etag.clone(),
                        })
                        .unwrap_or(SaveTarget::New);
                    self.start_document_save(tab, file, target, sender);
                } else {
                    if was_scratch {
                        self.pending_navigation.disconnect_pending = false;
                    }
                    if self.closing.preparation.is_some() {
                        self.cancel_close(root, sender);
                    }
                }
            }
            DocumentMsg::Loaded {
                tab: id,
                document_id,
                request,
                reload,
                result,
            } => {
                let Some(tab) = self.query_tab_mut(id) else {
                    return;
                };
                let Some(doc) = tab
                    .document
                    .as_mut()
                    .filter(|doc| doc.id == document_id && doc.request == Some(request))
                else {
                    return;
                };
                doc.request = None;
                tab.page.set_loading(false);
                tab.editor.emit(SqlEditorMsg::SetEditable(true));
                match result {
                    Ok(loaded) => {
                        doc.saved_text = loaded.text.clone();
                        doc.etag = loaded.etag;
                        doc.bom = loaded.bom;
                        doc.external_changed = false;
                        tab.editor_buffer.set_enable_undo(false);
                        tab.editor_buffer.set_text(&loaded.text);
                        tab.editor_buffer.set_enable_undo(true);
                        self.attach_document_monitor(id, sender);
                        self.persist_document(id, sender);
                        self.update_query_tab_title(id);
                    }
                    Err(error) => {
                        let file = doc.file.clone();
                        if !reload {
                            self.release_document_tab(id);
                            if self.tab_count() == 1 {
                                self.add_query_tab(widgets, sender);
                            }
                            self.close_query_tab(id, widgets);
                        }
                        show_document_open_failure(id, reload, file, error, root, sender);
                    }
                }
            }
            DocumentMsg::Saved { request, result } => {
                let Some(snapshot) = self.documents.saves.get(&request) else {
                    return;
                };
                let id = snapshot.tab;
                if !self.query_tabs.iter().any(|tab| {
                    tab.id == id
                        && tab.document.as_ref().map(|doc| &doc.id) == snapshot.document_id.as_ref()
                }) {
                    self.abort_document_save(request);
                    return;
                }
                match result {
                    Ok(etag) => {
                        let snapshot = self
                            .documents
                            .saves
                            .remove(&request)
                            .expect("save snapshot");
                        let old_file = self
                            .query_tabs
                            .iter()
                            .find(|tab| tab.id == id)
                            .and_then(|tab| tab.document.as_ref())
                            .and_then(|doc| doc.file.clone());
                        if let Some(old) = old_file
                            && old != snapshot.file
                        {
                            self.release_uri(&old, id);
                        }
                        let tab = self.query_tab_mut(id).expect("saved tab");
                        if let Some(doc) = tab.document.as_mut() {
                            doc.file = Some(snapshot.file);
                            doc.saved_text = snapshot.text;
                            doc.etag = etag;
                            doc.bom = snapshot.bom;
                            doc.external_changed = false;
                            doc.recovered_from = None;
                        } else {
                            tab.document = Some(SqlDocument {
                                file: Some(snapshot.file),
                                external_changed: false,
                                recovered_from: None,
                                monitor_generation: 0,
                                disk_check_request: None,
                                promotion_pending: true,
                                source_tab_id: snapshot.source_tab_id,
                                saved_text: snapshot.text,
                                etag,
                                bom: snapshot.bom,
                                id: Uuid::new_v4().to_string(),
                                origin: snapshot.origin,
                                revision: 0,
                                request: None,
                                monitor: None,
                            });
                        }
                        tab.page.set_loading(false);
                        let document_id = tab.document.as_ref().expect("saved document").id.clone();
                        self.claim_document(format!("document:{document_id}"), id);
                        self.attach_document_monitor(id, sender);
                        self.update_query_tab_title(id);
                        self.persist_document(id, sender);
                        self.schedule_session_save(sender);
                        if let Some(close) = &mut self.closing.preparation
                            && snapshot
                                .close_generation
                                .is_some_and(|generation| close.accepts_decision(generation, id))
                        {
                            close.remaining.pop();
                            self.advance_close(widgets, root, sender);
                        }
                    }
                    Err(FileError::Conflict | FileError::Deleted | FileError::Unverified) => {
                        let file = snapshot.file.clone();
                        let sender = sender.input_sender().clone();
                        glib::spawn_future_local(async move {
                            let result = sql_file::load(&file).await;
                            let _ = sender.send(WindowContentMsg::Document(
                                DocumentMsg::OverwriteVersion { request, result },
                            ));
                        });
                    }
                    Err(error) => {
                        if snapshot.document_id.is_none() {
                            self.pending_navigation.disconnect_pending = false;
                        }
                        self.abort_document_save(request);
                        self.cancel_close(root, sender);
                        show_document_save_failure(id, error, root, sender);
                    }
                }
            }
            DocumentMsg::OverwriteVersion { request, result } => {
                if !self.documents.saves.contains_key(&request) {
                    return;
                }
                show_document_save_conflict(request, result, root, sender);
            }
            DocumentMsg::OverwriteConfirmed { request, etag } => {
                let Some(snapshot) = self.documents.saves.get(&request) else {
                    return;
                };
                let (file, text, bom) =
                    (snapshot.file.clone(), snapshot.text.clone(), snapshot.bom);
                let sender = sender.input_sender().clone();
                glib::spawn_future_local(async move {
                    let target = if etag.is_some() {
                        SaveTarget::Overwrite { etag }
                    } else {
                        SaveTarget::New
                    };
                    let result = sql_file::save(&file, text, bom, target).await;
                    let _ = sender.send(WindowContentMsg::Document(DocumentMsg::Saved {
                        request,
                        result,
                    }));
                });
            }
            DocumentMsg::SaveAsRequestedAfterConflict(request) => {
                if let Some(snapshot) = self.documents.saves.get(&request) {
                    let id = snapshot.tab;
                    self.abort_document_save(request);
                    self.save_document(id, true, root, sender);
                }
            }
            DocumentMsg::SaveCancelled(request) => {
                if self
                    .documents
                    .saves
                    .get(&request)
                    .is_some_and(|snapshot| snapshot.document_id.is_none())
                {
                    self.pending_navigation.disconnect_pending = false;
                }
                self.abort_document_save(request);
                self.cancel_close(root, sender);
            }
            DocumentMsg::SaveTab(name, save_as) => {
                if self.close_preparation_pending() {
                    return;
                }
                if let Some(id) = query_id(name.as_deref()) {
                    self.save_document(id, save_as, root, sender);
                }
            }
            DocumentMsg::ReloadTab(name) => {
                if self.close_preparation_pending() {
                    return;
                }
                if let Some(id) = query_id(name.as_deref()) {
                    self.reload_document(id, root, sender, false);
                }
            }
            DocumentMsg::ReloadConfirmed {
                tab,
                document_id,
                revision,
                uri,
            } => {
                if self.close_preparation_pending() {
                    return;
                }
                if self.query_tabs.iter().any(|item| {
                    item.id == tab
                        && item.document.as_ref().is_some_and(|doc| {
                            doc.id == document_id
                                && doc.revision == revision
                                && doc.file.as_ref().is_some_and(|file| file.uri() == uri)
                        })
                }) {
                    self.reload_document(tab, root, sender, true);
                }
            }
            DocumentMsg::ExternalChange {
                tab,
                document_id,
                uri,
                generation,
            } => {
                let request = self.document_request();
                let Some(doc) = self
                    .query_tab_mut(tab)
                    .and_then(|tab| tab.document.as_mut())
                    .filter(|doc| doc.matches_monitor(&document_id, &uri, generation))
                else {
                    return;
                };
                let file = doc.file.clone().expect("monitor target");
                let baseline_etag = doc.etag.clone();
                doc.disk_check_request = Some(request);
                let sender = sender.input_sender().clone();
                glib::spawn_future_local(async move {
                    let result = file
                        .query_info_future(
                            "etag::value",
                            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                            glib::Priority::DEFAULT,
                        )
                        .await
                        .map(|info| info.etag().map(Into::into))
                        .map_err(|error| error.to_string());
                    let _ = sender.send(WindowContentMsg::Document(DocumentMsg::DiskChecked {
                        tab,
                        document_id,
                        uri,
                        generation,
                        request,
                        baseline_etag,
                        result,
                    }));
                });
            }
            DocumentMsg::DiskChecked {
                tab,
                document_id,
                uri,
                generation,
                request,
                baseline_etag,
                result,
            } => {
                let Some(doc) = self
                    .query_tab_mut(tab)
                    .and_then(|tab| tab.document.as_mut())
                    .filter(|doc| {
                        doc.matches_monitor(&document_id, &uri, generation)
                            && doc.disk_check_request == Some(request)
                            && doc.etag == baseline_etag
                    })
                else {
                    return;
                };
                doc.disk_check_request = None;
                doc.external_changed = result.is_err()
                    || result
                        .ok()
                        .flatten()
                        .is_none_or(|etag| Some(etag) != baseline_etag);
                self.update_query_tab_title(tab);
            }
            DocumentMsg::RecoveryValidated {
                tab: id,
                document_id,
                request,
                result,
            } => {
                let current = self.query_tab_sql(id).unwrap_or_default();
                let Some(tab) = self.query_tab_mut(id) else {
                    return;
                };
                let Some(doc) = tab
                    .document
                    .as_mut()
                    .filter(|doc| doc.id == document_id && doc.request == Some(request))
                else {
                    return;
                };
                let previous_file = doc.file.clone();
                if let Some(text) = doc.validate_recovery(&current, result) {
                    tab.editor_buffer.set_enable_undo(false);
                    tab.editor_buffer.set_text(&text);
                    tab.editor_buffer.set_enable_undo(true);
                }
                let detached = doc.file.is_none();
                doc.request = None;
                tab.page.set_loading(false);
                tab.editor.emit(SqlEditorMsg::SetEditable(true));
                if detached && let Some(file) = previous_file {
                    self.release_uri(&file, id);
                }
                self.attach_document_monitor(id, sender);
                self.update_query_tab_title(id);
                self.persist_document(id, sender);
            }
            DocumentMsg::SyncFileActions => {
                self.sync_file_actions(root, widgets);
            }
            DocumentMsg::FlushRecovery {
                tab,
                document_id,
                revision,
            } => {
                if self.query_tabs.iter().any(|item| {
                    item.id == tab
                        && item
                            .document
                            .as_ref()
                            .is_some_and(|doc| doc.id == document_id && doc.revision == revision)
                }) && !self
                    .closing
                    .preparation
                    .as_ref()
                    .is_some_and(|close| close.approved)
                {
                    self.persist_document(tab, sender);
                }
            }
            DocumentMsg::RecoveryLoaded { scope, result } => {
                if self
                    .documents
                    .pending_session
                    .as_ref()
                    .is_none_or(|pending| pending.scope != scope)
                {
                    return;
                }
                let pending = self
                    .documents
                    .pending_session
                    .take()
                    .expect("pending session restore");
                self.documents.recovery_pending = self.documents.recovery_pending.saturating_sub(1);
                if self.active_database_scope().as_ref() != Some(&scope) {
                    self.documents.restored.remove(&scope);
                    widgets.content_stack.set_sensitive(true);
                    return;
                }
                let mut session = pending.session;
                match result {
                    Ok(report) => {
                        let sources = promotion_sources(&report.records, &scope);
                        for id in &sources {
                            tabs::reserve_query_session_id(*id);
                        }
                        if let Some(session) = &mut session {
                            suppress_promoted_scratch_tabs(session, &scope, &report.records);
                        }
                        self.restore_session(session, widgets, sender);
                        for record in report.records {
                            if record.connection_id != scope.0 || record.database != scope.1 {
                                continue;
                            }
                            if self.query_tabs.iter().any(|tab| {
                                tab.document
                                    .as_ref()
                                    .is_some_and(|doc| doc.id == record.document_id)
                            }) {
                                continue;
                            }
                            let mut file = record.uri.as_ref().map(|uri| gio::File::for_uri(uri));
                            let id = self.next_query_tab_id;
                            if !self.claim_document(format!("document:{}", record.document_id), id)
                            {
                                continue;
                            }
                            if let Some(target) = &file
                                && !self.reserve_document(target, id)
                            {
                                file = None;
                            }
                            self.add_query_tab(widgets, sender);
                            let request = file.as_ref().map(|_| self.document_request());
                            let validation_file = file.clone();
                            let document_id = record.document_id.clone();
                            let tab = self.query_tab_mut(id).expect("recovery tab");
                            tab.editor_buffer.set_enable_undo(false);
                            tab.editor_buffer.set_text(&record.text);
                            tab.editor_buffer.set_enable_undo(true);
                            tab.document = Some(SqlDocument {
                                file,
                                external_changed: false,
                                recovered_from: record.uri,
                                monitor_generation: 0,
                                disk_check_request: None,
                                promotion_pending: false,
                                source_tab_id: record.source_tab_id,
                                saved_text: record.saved_text,
                                id: record.document_id,
                                origin: scope.clone(),
                                etag: record.etag,
                                bom: record.bom,
                                revision: 0,
                                request,
                                monitor: None,
                            });
                            self.update_query_tab_title(id);
                            if let Some(file) = validation_file
                                && let Some(request) = request
                            {
                                let tab = self.query_tab_mut(id).expect("recovery tab");
                                tab.page.set_loading(true);
                                tab.editor.emit(SqlEditorMsg::SetEditable(false));
                                let sender = sender.input_sender().clone();
                                glib::spawn_future_local(async move {
                                    let result = sql_file::load(&file).await;
                                    let _ = sender.send(WindowContentMsg::Document(
                                        DocumentMsg::RecoveryValidated {
                                            tab: id,
                                            document_id,
                                            request,
                                            result,
                                        },
                                    ));
                                });
                            } else {
                                self.persist_document(id, sender);
                            }
                        }
                        if let Err(error) = self.save_current_session(widgets) {
                            document_error(root, &gettext("Saving Tabs Failed"), &error);
                        }
                        if !report.warnings.is_empty() {
                            show_recovery_warnings(root, &report.warnings);
                        }
                    }
                    Err(error) => {
                        self.restore_session(session, widgets, sender);
                        self.documents.restored.remove(&scope);
                        document_error(root, &gettext("Restoring SQL Files Failed"), &error);
                    }
                }
                widgets.content_stack.set_sensitive(true);
                if self.documents.recovery_pending == 0
                    && self.shows_workspace()
                    && !self.documents.queued_files.is_empty()
                {
                    sender.input(WindowContentMsg::OpenSqlFileLocations(std::mem::take(
                        &mut self.documents.queued_files,
                    )));
                }
            }
            DocumentMsg::RecoveryStored {
                document_id,
                result,
            } => {
                if !self.query_tabs.iter().any(|tab| {
                    tab.document
                        .as_ref()
                        .is_some_and(|doc| doc.id == document_id)
                }) {
                    return;
                }
                if result.is_ok() {
                    self.documents.recovery_errors_shown.remove(&document_id);
                } else if !self
                    .documents
                    .recovery_errors_shown
                    .insert(document_id.clone())
                {
                    return;
                }
                let promotion_tab = self
                    .query_tabs
                    .iter()
                    .find(|tab| {
                        tab.document
                            .as_ref()
                            .is_some_and(|doc| doc.id == document_id && doc.promotion_pending)
                    })
                    .map(|tab| tab.id);
                match (promotion_tab, result) {
                    (Some(id), Ok(())) => {
                        self.query_tab_mut(id)
                            .and_then(|tab| tab.document.as_mut())
                            .expect("promoted document")
                            .promotion_pending = false;
                        if let Err(error) = self.save_current_session(widgets) {
                            document_error(root, &gettext("Saving Tabs Failed"), &error);
                        }
                    }
                    (Some(id), Err(error)) => {
                        show_promotion_recovery_failure(id, document_id, error, root, sender);
                    }
                    (None, Err(error)) => {
                        document_error(root, &gettext("Saving SQL Recovery Failed"), &error)
                    }
                    (None, Ok(())) => {}
                }
            }
            DocumentMsg::RetryPromotionRecovery { tab, document_id } => {
                self.documents.recovery_errors_shown.remove(&document_id);
                if self.query_tabs.iter().any(|item| {
                    item.id == tab
                        && item
                            .document
                            .as_ref()
                            .is_some_and(|doc| doc.id == document_id && doc.promotion_pending)
                }) {
                    self.persist_document(tab, sender);
                }
            }
            DocumentMsg::ContinueWithoutRecovery { tab, document_id } => {
                self.documents.recovery_errors_shown.remove(&document_id);
                if let Some(doc) = self
                    .query_tab_mut(tab)
                    .and_then(|tab| tab.document.as_mut())
                    .filter(|doc| doc.id == document_id)
                {
                    doc.promotion_pending = false;
                }
            }
        }
    }

    pub(super) fn resume_queued_sql_files(&mut self, sender: &ComponentSender<Self>) {
        if self.shows_workspace()
            && !self.documents.queued_files.is_empty()
            && self.active_database_switch_request_id.is_none()
        {
            sender.input(WindowContentMsg::OpenSqlFileLocations(std::mem::take(
                &mut self.documents.queued_files,
            )));
        }
    }

    fn abort_document_save(&mut self, request: u64) {
        if let Some(snapshot) = self.documents.saves.remove(&request) {
            let is_current = self
                .query_tabs
                .iter()
                .find(|tab| tab.id == snapshot.tab)
                .and_then(|tab| tab.document.as_ref())
                .is_some_and(|doc| doc.file.as_ref() == Some(&snapshot.file));
            if !is_current {
                self.release_uri(&snapshot.file, snapshot.tab);
            }
            if let Some(tab) = self.query_tab_mut(snapshot.tab) {
                tab.page.set_loading(false);
            }
        }
    }
}

pub(super) fn query_id(name: Option<&str>) -> Option<u64> {
    name?.strip_prefix("query-tab-")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn document() -> SqlDocument {
        SqlDocument {
            file: Some(gio::File::for_path("/tmp/codd-document-test.sql")),
            external_changed: false,
            recovered_from: None,
            monitor_generation: 0,
            disk_check_request: None,
            promotion_pending: false,
            source_tab_id: None,
            saved_text: "  select 1;\r\n\n ".into(),
            id: uuid::Uuid::new_v4().to_string(),
            origin: ("original-connection".into(), "original-database".into()),
            etag: Some("original-etag".into()),
            bom: true,
            revision: 7,
            request: None,
            monitor: None,
        }
    }

    #[test]
    fn dirty_comparison_is_exact_and_undo_can_restore_clean_state() {
        let doc = document();
        assert!(!doc.is_dirty(&doc.saved_text));
        assert!(doc.is_dirty(doc.saved_text.trim()));
        assert!(doc.is_dirty(&doc.saved_text.replace("\r\n", "\n")));
        assert!(doc.is_dirty(""));
        assert!(!doc.is_dirty(&doc.saved_text.clone()));
    }

    #[test]
    fn completing_save_of_snapshot_does_not_mark_later_edits_clean() {
        let mut doc = document();
        let snapshot = "select 2;\n".to_string();
        let current = "select 3;\n";
        doc.saved_text = snapshot.clone();
        doc.etag = Some("saved-etag".into());
        assert!(doc.is_dirty(current));
        assert!(!doc.is_dirty(&snapshot));
        assert_eq!(doc.revision, 7);
    }

    #[test]
    fn recovery_preserves_exact_text_baseline_and_original_scope() {
        let doc = document();
        let current = "\r\n  select 'é';\n\n ";
        let record = doc.recovery_record(current.into());
        assert_eq!(record.text, current);
        assert_eq!(record.saved_text, doc.saved_text);
        assert_eq!(record.document_id, doc.id);
        assert_eq!(record.connection_id, "original-connection");
        assert_eq!(record.database, "original-database");
        assert_eq!(record.etag, doc.etag);
        assert!(record.bom);
        assert_eq!(
            record.uri.as_deref(),
            Some("file:///tmp/codd-document-test.sql")
        );
    }

    #[test]
    fn conflicting_recovery_keeps_uuid_and_exact_text_without_a_writable_target() {
        let mut doc = document();
        let id = doc.id.clone();
        doc.recovered_from = doc.file.take().map(|file| file.uri().to_string());
        let text = "  select 'recovered';\r\n\n ";
        let record = doc.recovery_record(text.into());
        assert_eq!(record.document_id, id);
        assert_eq!(record.text, text);
        assert_eq!(record.saved_text, doc.saved_text);
        assert!(record.uri.is_none());
        assert!(doc.is_dirty(text));
        assert!(doc.is_dirty(&doc.saved_text));
        assert!(doc.tooltip().contains("codd-document-test.sql"));
    }

    #[test]
    fn clean_recovery_reads_current_disk_instead_of_presenting_a_stale_baseline() {
        let mut doc = document();
        let current = doc.saved_text.clone();
        let latest = "select 'external edit';\r\n ";
        let replacement = doc.validate_recovery(
            &current,
            Ok(LoadedSqlFile {
                text: latest.into(),
                etag: Some("latest".into()),
                bom: false,
            }),
        );
        assert_eq!(replacement.as_deref(), Some(latest));
        assert_eq!(doc.saved_text, latest);
        assert_eq!(doc.etag.as_deref(), Some("latest"));
        assert!(!doc.bom);
        assert!(!doc.is_dirty(latest));
        assert!(!doc.external_changed);
    }

    #[test]
    fn unavailable_clean_recovery_becomes_an_unsaved_copy_not_discardable_clean_text() {
        let mut doc = document();
        let text = doc.saved_text.clone();
        let id = doc.id.clone();
        assert!(
            doc.validate_recovery(&text, Err(FileError::Deleted))
                .is_none()
        );
        assert!(doc.file.is_none());
        assert!(doc.is_dirty(&text));
        assert!(doc.external_changed);
        let record = doc.recovery_record(text.clone());
        assert_eq!(record.document_id, id);
        assert_eq!(record.text, text);
        assert!(record.uri.is_none());
    }

    #[test]
    fn dirty_recovery_never_adopts_external_text_or_a_conflicting_etag() {
        let mut doc = document();
        let baseline = doc.saved_text.clone();
        let replacement = doc.validate_recovery(
            "unsaved recovery\n",
            Ok(LoadedSqlFile {
                text: "external edit".into(),
                etag: Some("conflicting".into()),
                bom: false,
            }),
        );
        assert!(replacement.is_none());
        assert_eq!(doc.saved_text, baseline);
        assert_eq!(doc.etag.as_deref(), Some("original-etag"));
        assert!(doc.external_changed);
        assert!(
            doc.validate_recovery("unsaved recovery\n", Err(FileError::Deleted))
                .is_none()
        );
        assert_eq!(doc.saved_text, baseline);
        assert!(doc.external_changed);
    }

    #[test]
    fn unchanged_disk_can_refresh_etag_without_replacing_dirty_recovery() {
        let mut doc = document();
        let replacement = doc.validate_recovery(
            "new unsaved text",
            Ok(LoadedSqlFile {
                text: doc.saved_text.clone(),
                etag: Some("equivalent-version".into()),
                bom: doc.bom,
            }),
        );
        assert!(replacement.is_none());
        assert_eq!(doc.etag.as_deref(), Some("equivalent-version"));
        assert!(!doc.external_changed);
        assert!(doc.is_dirty("new unsaved text"));
    }

    #[test]
    fn monitor_identity_rejects_old_uri_generation_and_document() {
        let mut doc = document();
        doc.monitor_generation = 4;
        let old_uri = doc.file.as_ref().unwrap().uri().to_string();
        assert!(doc.matches_monitor(&doc.id, &old_uri, 4));
        assert!(!doc.matches_monitor("other-document", &old_uri, 4));
        assert!(!doc.matches_monitor(&doc.id, &old_uri, 3));
        doc.file = Some(gio::File::for_path("/tmp/codd-saved-as.sql"));
        assert!(!doc.matches_monitor(&doc.id, &old_uri, 4));
        doc.external_changed = true;
        assert!(doc.tooltip().contains("changed on disk"));
        assert!(doc.is_dirty("new edit"));
        assert!(doc.tooltip().contains("changed on disk"));
    }

    #[test]
    fn chooser_identity_rejects_replaced_tabs_promotions_and_scope_changes() {
        let scope = ("connection".into(), "database".into());
        let scratch = SaveChooserIdentity {
            tab: 8,
            document_id: None,
            scope: scope.clone(),
        };
        assert!(scratch.matches(8, None, Some(&scope)));
        assert!(!scratch.matches(9, None, Some(&scope)));
        assert!(!scratch.matches(8, Some("promoted"), Some(&scope)));
        assert!(!scratch.matches(8, None, Some(&("connection".into(), "other".into()))));
        assert!(!scratch.matches(8, None, None));
        let file = SaveChooserIdentity {
            document_id: Some("file-document".into()),
            ..scratch
        };
        assert!(file.matches(8, Some("file-document"), Some(&scope)));
        assert!(!file.matches(8, Some("replacement"), Some(&scope)));
    }

    #[test]
    fn chooser_and_inflight_save_both_exclude_navigation_before_promotion() {
        let mut workspace = DocumentWorkspace::default();
        assert!(!workspace.has_pending_operations(false));
        workspace.chooser_open = true;
        assert!(workspace.has_pending_operations(false));
        workspace.chooser_open = false;
        workspace.saves.insert(
            1,
            SaveSnapshot {
                tab: 5,
                document_id: None,
                file: gio::File::for_path("/tmp/query.sql"),
                text: "snapshot".into(),
                bom: false,
                origin: ("connection".into(), "database".into()),
                close_generation: None,
                source_tab_id: Some(5),
            },
        );
        assert!(workspace.has_pending_operations(false));
        workspace.saves.clear();
        assert!(workspace.has_pending_operations(true));
        workspace.recovery_pending = 1;
        assert!(workspace.has_pending_operations(false));
        workspace.recovery_pending = 0;
        assert!(!workspace.has_pending_operations(false));
    }

    #[test]
    fn promotion_keeps_navigation_and_session_migration_blocked_until_recovery_is_durable() {
        let mut doc = document();
        assert!(!doc.is_busy());
        doc.promotion_pending = true;
        assert!(doc.is_busy());
        doc.promotion_pending = false;
        assert!(!doc.is_busy());
        doc.request = Some(12);
        assert!(doc.is_busy());
        doc.request = None;
        assert!(!doc.is_busy());
    }

    #[test]
    fn promotion_provenance_survives_recovery_and_save_as_without_using_runtime_ids() {
        let mut doc = document();
        doc.source_tab_id = Some(42);
        let record = doc.recovery_record("newer edits".into());
        let persisted = serde_json::to_string(&record).unwrap();
        let restored: RecoveryRecord = serde_json::from_str(&persisted).unwrap();
        assert_eq!(restored.source_tab_id, Some(42));
        assert_eq!(restored.text, "newer edits");
        doc.file = Some(gio::File::for_path("/tmp/renamed-query.sql"));
        assert_eq!(
            doc.recovery_record("still newer edits".into())
                .source_tab_id,
            Some(42)
        );
    }

    #[test]
    fn async_save_snapshot_preserves_later_edits_and_original_session_identity() {
        struct TestFile(std::path::PathBuf);
        impl Drop for TestFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }

        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                context.block_on(async {
                    let temporary = TestFile(
                        std::env::temp_dir()
                            .join(format!("codd-document-{}.sql", uuid::Uuid::new_v4())),
                    );
                    let file = gio::File::for_path(&temporary.0);
                    let mut doc = document();
                    let snapshot = SaveSnapshot {
                        tab: 9001,
                        document_id: None,
                        file: file.clone(),
                        text: "  select 1;\r\n ".into(),
                        bom: true,
                        origin: doc.origin.clone(),
                        close_generation: None,
                        source_tab_id: Some(42),
                    };
                    let save = sql_file::save(
                        &snapshot.file,
                        snapshot.text.clone(),
                        snapshot.bom,
                        SaveTarget::New,
                    );
                    let current = "  select 2;\r\n\n newer edits ";
                    let etag = save.await.unwrap();
                    doc.file = Some(file.clone());
                    doc.saved_text = snapshot.text;
                    doc.etag = etag;
                    doc.bom = snapshot.bom;
                    doc.source_tab_id = snapshot.source_tab_id;
                    let record = doc.recovery_record(current.into());
                    let loaded = sql_file::load(&file).await.unwrap();
                    assert_eq!(loaded.text, doc.saved_text);
                    assert_eq!(record.text, current);
                    assert_eq!(record.source_tab_id, Some(42));
                    assert_ne!(record.source_tab_id, Some(snapshot.tab));
                    assert!(doc.is_dirty(current));
                    assert!(loaded.bom);
                })
            })
            .unwrap();
    }
}
