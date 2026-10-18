use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use super::{
    SqlEditorMsg, WindowContent, WindowContentMsg, WindowContentOutput, WindowContentWidgets, tabs,
    with_window_content_subscribers,
};
use crate::db::postgres::PostgresConnection;
use crate::models::connection::ConnectionDetails;
use crate::models::session::{SavedSession, SavedSessionTab};
use crate::sql_file::{self, FileError, LoadedSqlFile, SaveTarget};
use crate::state::document_store::{self, RecoveryLoad, RecoveryRecord};
use crate::state::session_store;
use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::Sender;
use relm4::gtk::{self, gio, glib};
use relm4::prelude::*;
use uuid::Uuid;

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
    close: Option<ClosePreparation>,
    close_dialog: Option<adw::AlertDialog>,
    next_request: u64,
    saves: HashMap<u64, SaveSnapshot>,
    chooser_open: bool,
    chooser: Option<gtk::FileChooserNative>,
    chooser_request: Option<u64>,
    recovery_pending: usize,
    recovery_errors_shown: HashSet<String>,
    chooser_identity: Option<SaveChooserIdentity>,
    pub(super) disconnect_pending: bool,
    pub(super) deferred_switch: Option<WindowContentMsg>,
    pub(super) deferred_connection: Option<(ConnectionDetails, PostgresConnection)>,
    pending_session: Option<PendingSession>,
}

struct PendingSession {
    scope: (String, String),
    session: Option<SavedSession>,
}

fn promotion_sources(records: &[RecoveryRecord], scope: &(String, String)) -> HashSet<u64> {
    records
        .iter()
        .filter(|record| record.connection_id == scope.0 && record.database == scope.1)
        .filter_map(|record| record.source_tab_id)
        .collect()
}

fn suppress_promoted_scratch_tabs(
    session: &mut SavedSession,
    scope: &(String, String),
    records: &[RecoveryRecord],
) {
    if (&session.connection_id, &session.database) != (&scope.0, &scope.1) {
        return;
    }
    session.tabs.retain(|tab| {
        let SavedSessionTab::Query { id, sql, .. } = tab else {
            return true;
        };
        !records.iter().any(|record| {
            record.connection_id == scope.0
                && record.database == scope.1
                && record.source_tab_id == Some(*id)
                && (sql.trim() == record.saved_text.trim() || sql.trim() == record.text.trim())
        })
    });
    if session
        .active_tab
        .is_some_and(|id| !session.tabs.iter().any(|tab| tab.id() == id))
    {
        session.active_tab = None;
    }
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

struct ClosePreparation {
    targets: Option<Vec<String>>,
    remaining: Vec<u64>,
    discarded: Vec<u64>,
    approved: bool,
    generation: u64,
    committing: bool,
    cancelling: bool,
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

    fn accepts_decision(&self, generation: u64, tab: u64) -> bool {
        !self.approved
            && !self.committing
            && !self.cancelling
            && self.generation == generation
            && self.remaining.last() == Some(&tab)
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
    CloseDecision {
        generation: u64,
        tab: u64,
        decision: String,
    },
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
    CleanupFinished {
        generation: u64,
        result: Result<(), String>,
    },
    CancellationFinished {
        generation: u64,
        result: Result<(), String>,
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

enum RecoveryJob {
    Save(String, Sender<WindowContentMsg>),
    Load((String, String), Sender<WindowContentMsg>),
    Remove(Vec<String>, Option<(u64, Sender<WindowContentMsg>)>),
    Restore(Vec<RecoveryRecord>, u64, Sender<WindowContentMsg>),
}

static PENDING_RECOVERY: OnceLock<Mutex<HashMap<String, RecoveryRecord>>> = OnceLock::new();

fn pending_recovery() -> &'static Mutex<HashMap<String, RecoveryRecord>> {
    PENDING_RECOVERY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn recovery_worker() -> &'static mpsc::Sender<RecoveryJob> {
    static WORKER: OnceLock<mpsc::Sender<RecoveryJob>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for job in rx {
                match job {
                    RecoveryJob::Save(id, sender) => {
                        let record = pending_recovery()
                            .lock()
                            .expect("recovery queue")
                            .remove(&id);
                        if let Some(record) = record {
                            let result = document_store::save(&record);
                            let _ = sender.send(WindowContentMsg::Document(
                                DocumentMsg::RecoveryStored {
                                    document_id: id,
                                    result,
                                },
                            ));
                        }
                    }
                    RecoveryJob::Load(scope, sender) => {
                        let result = document_store::load_for_database(&scope.0, &scope.1);
                        let _ =
                            sender.send(WindowContentMsg::Document(DocumentMsg::RecoveryLoaded {
                                scope,
                                result,
                            }));
                    }
                    RecoveryJob::Remove(ids, completion) => {
                        let result = ids.iter().try_for_each(|id| document_store::remove(id));
                        if let Some((generation, sender)) = completion {
                            let _ = sender.send(WindowContentMsg::Document(
                                DocumentMsg::CleanupFinished { generation, result },
                            ));
                        }
                    }
                    RecoveryJob::Restore(records, generation, sender) => {
                        let mut result = Ok(());
                        for record in records {
                            if let Err(error) = document_store::save(&record) {
                                result = Err(error);
                            }
                        }
                        let _ = sender.send(WindowContentMsg::Document(
                            DocumentMsg::CancellationFinished { generation, result },
                        ));
                    }
                }
            }
        });
        tx
    })
}

impl WindowContent {
    #[cfg(test)]
    pub(crate) fn install_protocol_test_document(&mut self) -> (u64, RecoveryRecord) {
        let mut doc = tests::document();
        doc.id = Uuid::new_v4().to_string();
        doc.origin = (format!("protocol-{}", doc.id), "postgres".into());
        doc.file = None;
        let text = "  select 'unsaved protocol test';\r\n\n ";
        let tab = self.query_tabs.last_mut().expect("initial scratch tab");
        doc.source_tab_id = Some(tab.session_id);
        let record = doc.recovery_record(text.into());
        tab.editor_buffer.set_text(text);
        tab.document = Some(doc);
        (tab.id, record)
    }

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

    pub fn close_preparation_pending(&self) -> bool {
        self.documents.close.is_some()
    }

    pub(super) fn document_input_blocked(&self, msg: &WindowContentMsg) -> bool {
        (self.close_preparation_pending() || self.documents.pending_session.is_some())
            && !matches!(
                msg,
                WindowContentMsg::Document(_)
                    | WindowContentMsg::CancelClosePreparation
                    | WindowContentMsg::CommitClose
                    | WindowContentMsg::PrepareClose
                    | WindowContentMsg::SaveSession
                    | WindowContentMsg::QueryTabTitleChanged(_)
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

    fn release_document_tab(&self, tab: u64) {
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

    pub(super) fn choose_sql_files(
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

    fn save_document(
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
                self.documents.disconnect_pending = false;
            }
            return;
        }
        if !self.reserve_document(&file, id) {
            if scratch {
                self.documents.disconnect_pending = false;
            }
            if self.documents.close.is_some() {
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
        let close_generation = self.documents.close.as_ref().map(|close| close.generation);
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

    pub(super) fn document_text_changed(&mut self, id: u64, sender: &ComponentSender<Self>) {
        let Some(doc) = self.query_tab_mut(id).and_then(|tab| tab.document.as_mut()) else {
            return;
        };
        if doc.request.is_some() {
            return;
        }
        doc.revision += 1;
        let document_id = doc.id.clone();
        let revision = doc.revision;
        let sender = sender.input_sender().clone();
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            let _ = sender.send(WindowContentMsg::Document(DocumentMsg::FlushRecovery {
                tab: id,
                document_id,
                revision,
            }));
        });
    }

    fn persist_document(&self, id: u64, sender: &ComponentSender<Self>) {
        let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let Some(doc) = &tab.document else {
            return;
        };
        let record = doc.recovery_record(self.query_tab_sql(id).unwrap_or_default());
        let id = record.document_id.clone();
        let already_queued = pending_recovery()
            .lock()
            .expect("recovery queue")
            .insert(id.clone(), record)
            .is_some();
        if !already_queued {
            let _ = recovery_worker().send(RecoveryJob::Save(id, sender.input_sender().clone()));
        }
    }

    pub(super) fn restore_documents(
        &mut self,
        mut session: Option<SavedSession>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(scope) = self.active_database_scope() else {
            return;
        };
        if self.documents.restored.insert(scope.clone()) {
            self.documents.pending_session = Some(PendingSession {
                scope: scope.clone(),
                session,
            });
            widgets.content_stack.set_sensitive(false);
            self.documents.recovery_pending += 1;
            let _ = recovery_worker().send(RecoveryJob::Load(scope, sender.input_sender().clone()));
        } else {
            let records: Vec<_> = self
                .query_tabs
                .iter()
                .filter_map(|tab| {
                    let doc = tab.document.as_ref()?;
                    (doc.origin == scope).then(|| {
                        doc.recovery_record(self.query_tab_sql(tab.id).unwrap_or_default())
                    })
                })
                .collect();
            if let Some(session) = &mut session {
                suppress_promoted_scratch_tabs(session, &scope, &records);
            }
            self.restore_session(session, widgets, sender);
        }
    }

    pub(super) fn document_close_targets(&self, keep: Option<&str>) -> Vec<String> {
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

    pub(super) fn begin_document_close(
        &mut self,
        targets: Option<Vec<String>>,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(close) = &self.documents.close {
            if targets.is_none() && close.targets.is_some() {
                self.cancel_document_close(root, sender);
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
        let generation = self.document_request();
        self.documents.close = Some(ClosePreparation {
            targets,
            remaining,
            discarded: Vec::new(),
            approved: false,
            generation,
            committing: false,
            cancelling: false,
            committed: false,
        });
        for tab in &self.query_tabs {
            self.persist_document(tab.id, sender);
        }
        root.set_sensitive(false);
        self.advance_document_close(widgets, root, sender);
    }

    fn advance_document_close(
        &mut self,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let Some(close) = &mut self.documents.close else {
            return;
        };
        if close.cancelling || close.committing || close.committed {
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
                let _ = sender.send(WindowContentMsg::Document(DocumentMsg::CloseDecision {
                    generation,
                    tab: id,
                    decision: decision.into(),
                }));
            });
            self.documents.close_dialog = Some(dialog.clone());
            present_document_dialog(&dialog, root);
        } else {
            close.approved = true;
            if close.targets.is_none() {
                let _ = sender.output(WindowContentOutput::ClosePrepared);
            } else {
                self.commit_document_close(widgets, root, sender);
            }
        }
    }

    pub(super) fn cancel_document_close(
        &mut self,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let Some(close) = &mut self.documents.close else {
            return;
        };
        let Some(generation) = close.start_cancellation() else {
            return;
        };
        if let Some(dialog) = self.documents.close_dialog.take() {
            dialog.close();
        }
        if let Some(chooser) = self.documents.chooser.take() {
            chooser.destroy();
        }
        self.documents.chooser_open = false;
        self.documents.chooser_request = None;
        self.documents.chooser_identity = None;
        root.set_sensitive(false);
        let records = self
            .query_tabs
            .iter()
            .filter_map(|tab| {
                tab.document
                    .as_ref()
                    .map(|doc| doc.recovery_record(self.query_tab_sql(tab.id).unwrap_or_default()))
            })
            .collect();
        let _ = recovery_worker().send(RecoveryJob::Restore(
            records,
            generation,
            sender.input_sender().clone(),
        ));
    }

    pub(super) fn commit_document_close(
        &mut self,
        widgets: &WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        let Some(close) = &self.documents.close else {
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
        let whole_window = close.targets.is_none();
        let records: Vec<_> = self
            .query_tabs
            .iter()
            .filter(|tab| {
                close
                    .targets
                    .as_ref()
                    .is_none_or(|targets| targets.contains(&format!("query-tab-{}", tab.id)))
            })
            .filter_map(|tab| {
                let doc = tab.document.as_ref()?;
                doc.source_tab_id
                    .map(|_| doc.recovery_record(self.query_tab_sql(tab.id).unwrap_or_default()))
            })
            .collect();
        let result = (|| {
            if whole_window {
                self.checkpoint_current_session(widgets)?;
            } else {
                self.save_current_session(widgets)?;
            }
            let scopes: HashSet<_> = records
                .iter()
                .map(|record| (record.connection_id.clone(), record.database.clone()))
                .collect();
            for scope in scopes {
                if let Some(mut session) = session_store::load(&scope.0, &scope.1) {
                    suppress_promoted_scratch_tabs(&mut session, &scope, &records);
                    session_store::save(&session).map_err(|error| error.to_string())?;
                }
            }

            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            self.cancel_document_close(root, sender);
            document_error(root, &gettext("Saving Tabs Failed"), &error);
            return;
        }
        let close = self.documents.close.as_mut().expect("prepared close");
        close.committing = true;
        let ids = self
            .query_tabs
            .iter()
            .filter(|tab| {
                close
                    .targets
                    .as_ref()
                    .is_none_or(|names| names.contains(&format!("query-tab-{}", tab.id)))
            })
            .filter_map(|tab| tab.document.as_ref().map(|doc| doc.id.clone()))
            .collect();
        let _ = recovery_worker().send(RecoveryJob::Remove(
            ids,
            Some((close.generation, sender.input_sender().clone())),
        ));
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
            let document_id = doc.id.clone();
            let revision = doc.revision;
            let uri = file.uri().to_string();
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
                        .documents
                        .close
                        .as_ref()
                        .is_some_and(|close| close.cancelling || close.approved)
                {
                    if was_scratch {
                        self.documents.disconnect_pending = false;
                    }
                    self.cancel_document_close(root, sender);
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
                        self.documents.disconnect_pending = false;
                    }
                    if self.documents.close.is_some() {
                        self.cancel_document_close(root, sender);
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
                        let dialog = adw::AlertDialog::builder()
                            .heading(gettext("Opening SQL File Failed"))
                            .body(error.to_string())
                            .build();
                        dialog.add_responses(&[
                            ("cancel", &gettext("Cancel")),
                            ("retry", &gettext("Retry")),
                        ]);
                        dialog.set_close_response("cancel");
                        let sender = sender.input_sender().clone();
                        dialog.connect_response(Some("retry"), move |_, _| {
                            if reload {
                                let _ = sender.send(WindowContentMsg::Document(
                                    DocumentMsg::ReloadTab(Some(format!("query-tab-{id}"))),
                                ));
                            } else {
                                let _ = sender.send(WindowContentMsg::OpenSqlFileLocations(vec![
                                    file.clone().expect("loaded file"),
                                ]));
                            }
                        });
                        present_document_dialog(&dialog, root);
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
                        if let Some(close) = &mut self.documents.close
                            && snapshot
                                .close_generation
                                .is_some_and(|generation| close.accepts_decision(generation, id))
                        {
                            close.remaining.pop();
                            self.advance_document_close(widgets, root, sender);
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
                            self.documents.disconnect_pending = false;
                        }
                        self.abort_document_save(request);
                        self.cancel_document_close(root, sender);
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
                }
            }
            DocumentMsg::OverwriteVersion { request, result } => {
                if !self.documents.saves.contains_key(&request) {
                    return;
                }
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
                    dialog
                        .set_response_appearance("overwrite", adw::ResponseAppearance::Destructive);
                }
                if deleted {
                    dialog.add_response("recreate", &gettext("Recreate File"));
                    dialog
                        .set_response_appearance("recreate", adw::ResponseAppearance::Destructive);
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
                        let _ = sender.send(WindowContentMsg::Document(
                            DocumentMsg::SaveCancelled(request),
                        ));
                    }
                });
                present_document_dialog(&dialog, root);
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
                    self.documents.disconnect_pending = false;
                }
                self.abort_document_save(request);
                self.cancel_document_close(root, sender);
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
            DocumentMsg::CloseDecision {
                generation,
                tab,
                decision,
            } => {
                let Some(close) = &mut self.documents.close else {
                    return;
                };
                if !close.accepts_decision(generation, tab) {
                    return;
                }
                self.documents.close_dialog = None;
                match decision.as_str() {
                    "discard" => {
                        close.discarded.push(tab);
                        close.remaining.pop();
                        self.advance_document_close(widgets, root, sender);
                    }
                    "save" => self.save_document(tab, false, root, sender),
                    _ => self.cancel_document_close(root, sender),
                }
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
                    .documents
                    .close
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
            DocumentMsg::CleanupFinished { generation, result } => {
                let Some(close) = &self.documents.close else {
                    return;
                };
                if !close.accepts_cleanup(generation) {
                    return;
                }
                if let Err(error) = result {
                    if let Some(close) = &mut self.documents.close {
                        close.committing = false;
                    }
                    self.cancel_document_close(root, sender);
                    document_error(root, &gettext("Closing SQL Files Failed"), &error);
                    return;
                }
                let close = self.documents.close.take().expect("prepared close");
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
                    self.documents.close = Some(ClosePreparation {
                        committed: true,
                        ..close
                    });
                    let _ = sender.output(WindowContentOutput::CloseCommitted);
                }
            }
            DocumentMsg::CancellationFinished { generation, result } => {
                if !self
                    .documents
                    .close
                    .as_ref()
                    .is_some_and(|close| close.generation == generation && close.cancelling)
                {
                    return;
                }
                self.documents.close = None;
                root.set_sensitive(true);
                let _ = sender.output(WindowContentOutput::CloseCancelled);
                if let Err(error) = result {
                    document_error(root, &gettext("Restoring SQL Recovery Failed"), &error);
                }
            }
        }
    }

    pub(super) fn resume_document_navigation(
        &mut self,
        widgets: &mut WindowContentWidgets,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
    ) {
        if !self.file_operation_pending() && !self.close_preparation_pending() {
            if self.documents.disconnect_pending {
                self.documents.deferred_switch = None;
                self.documents.deferred_connection = None;
                self.show_start_screen(widgets);
                return;
            }
            if let Some((details, postgres)) = self.documents.deferred_connection.take() {
                self.handle_connected(details, postgres, widgets, sender, root);
                return;
            }
            if let Some(msg) = self.documents.deferred_switch.take() {
                sender.input(msg);
                return;
            }
            if self.shows_workspace()
                && !self.documents.queued_files.is_empty()
                && self.active_database_switch_request_id.is_none()
            {
                sender.input(WindowContentMsg::OpenSqlFileLocations(std::mem::take(
                    &mut self.documents.queued_files,
                )));
            }
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

fn query_id(name: Option<&str>) -> Option<u64> {
    name?.strip_prefix("query-tab-")?.parse().ok()
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

fn present_document_dialog(dialog: &adw::AlertDialog, root: &adw::ToolbarView) {
    let parent = root.root().and_downcast::<gtk::Window>();
    dialog.present(parent.as_ref());
}

fn document_error(root: &adw::ToolbarView, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();
    dialog.add_response("close", &gettext("Close"));
    dialog.set_close_response("close");
    present_document_dialog(&dialog, root);
}

fn show_recovery_warnings(root: &adw::ToolbarView, warnings: &[String]) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::session::SavedSessionTabId;

    #[test]
    #[ignore = "requires a display and isolated XDG directories; run in its own process"]
    fn gtk_sql_file_editing() {
        use crate::models::connection::ConnectionForm;
        use std::cell::Ref;
        use std::fs;
        use std::time::{Duration, Instant};

        #[track_caller]
        fn until(mut ready: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !ready() {
                assert!(
                    Instant::now() < deadline,
                    "SQL file operation did not settle"
                );
                glib::MainContext::default().iteration(false);
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn edit(buffer: &sourceview5::Buffer, text: &str) {
            buffer.begin_user_action();
            buffer.insert(&mut buffer.end_iter(), text);
            buffer.end_user_action();
        }

        fn bytes(text: &str, bom: bool) -> Vec<u8> {
            let mut bytes = if bom {
                vec![0xef, 0xbb, 0xbf]
            } else {
                Vec::new()
            };
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }

        fn doc(content: &Controller<WindowContent>, id: u64) -> Ref<'_, SqlDocument> {
            Ref::map(content.model(), |model| {
                model
                    .query_tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .unwrap()
                    .document
                    .as_ref()
                    .unwrap()
            })
        }

        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let _app = relm4::RelmApp::<WindowContentMsg>::new(crate::config::APP_ID);
        sourceview5::init();
        crate::register_app_resources();
        let application = relm4::main_adw_application();
        application.register(gio::Cancellable::NONE).unwrap();
        let content = WindowContent::builder().launch(()).detach();
        let root = adw::ApplicationWindow::builder()
            .application(&application)
            .content(content.widget())
            .build();
        root.present();
        let details = ConnectionForm {
            name: "File Editing Test".into(),
            username: "test".into(),
            ..ConnectionForm::default()
        }
        .validate()
        .unwrap();
        {
            let mut parts = content.state().get_mut();
            parts.model.visible_page = super::super::VisiblePage::Workspace;
            parts.model.state.active_database = Some(details.saved.database.clone());
            parts.model.state.active_connection = Some(details.saved);
        }

        let directory =
            Directory(glib::user_data_dir().join(format!("file-editing-{}", uuid::Uuid::new_v4())));
        fs::create_dir(&directory.0).unwrap();
        let path = directory.0.join("query.sql");
        let file = gio::File::for_path(&path);
        let initial = "  select 'é';\r\n\r\n \n";
        fs::write(&path, bytes(initial, true)).unwrap();
        content.emit(WindowContentMsg::OpenSqlFileLocations(vec![file.clone()]));
        until(|| {
            content.model().query_tabs.iter().any(|tab| {
                tab.document
                    .as_ref()
                    .is_some_and(|doc| doc.file.as_ref() == Some(&file) && doc.request.is_none())
            })
        });
        let id = content
            .model()
            .query_tabs
            .iter()
            .find(|tab| {
                tab.document
                    .as_ref()
                    .is_some_and(|doc| doc.file.as_ref() == Some(&file))
            })
            .unwrap()
            .id;
        let (buffer, page) = {
            let model = content.model();
            let tab = model.query_tabs.iter().find(|tab| tab.id == id).unwrap();
            (tab.editor_buffer.clone(), tab.page.clone())
        };
        assert_eq!(content.model().query_tab_sql(id).as_deref(), Some(initial));
        assert_eq!(page.title().as_str(), "query.sql");
        assert!(page.indicator_icon().is_none());
        assert!(!buffer.can_undo());

        edit(&buffer, "-- before save\r\n");
        let snapshot = content.model().query_tab_sql(id).unwrap();
        content.emit(WindowContentMsg::SaveSqlFile);
        until(|| !content.model().documents.saves.is_empty());
        assert_eq!(
            content
                .model()
                .documents
                .saves
                .values()
                .next()
                .unwrap()
                .text,
            snapshot
        );
        edit(&buffer, "-- typed while saving\r\n");
        let later = content.model().query_tab_sql(id).unwrap();
        until(|| content.model().documents.saves.is_empty());
        assert_eq!(fs::read(&path).unwrap(), bytes(&snapshot, true));
        assert_eq!(
            content.model().query_tab_sql(id).as_deref(),
            Some(later.as_str())
        );
        assert_eq!(doc(&content, id).saved_text, snapshot);
        assert!(doc(&content, id).is_dirty(&later));
        assert!(page.indicator_icon().is_some());

        buffer.undo();
        until(|| page.indicator_icon().is_none());
        assert_eq!(
            content.model().query_tab_sql(id).as_deref(),
            Some(snapshot.as_str())
        );
        buffer.redo();
        until(|| page.indicator_icon().is_some());
        assert_eq!(
            content.model().query_tab_sql(id).as_deref(),
            Some(later.as_str())
        );
        content.emit(WindowContentMsg::SaveSqlFile);
        until(|| doc(&content, id).saved_text == later);
        assert_eq!(fs::read(&path).unwrap(), bytes(&later, true));
        assert!(page.indicator_icon().is_none());
        buffer.undo();
        until(|| page.indicator_icon().is_some());
        assert_eq!(
            content.model().query_tab_sql(id).as_deref(),
            Some(snapshot.as_str())
        );
        buffer.redo();
        until(|| page.indicator_icon().is_none());
        assert_eq!(
            content.model().query_tab_sql(id).as_deref(),
            Some(later.as_str())
        );
        glib::MainContext::default().block_on(glib::timeout_future(Duration::from_millis(150)));
        assert!(!doc(&content, id).external_changed);

        let external = "select 'external editor version without BOM';\r\n\r\n";
        fs::write(&path, external.as_bytes()).unwrap();
        until(|| doc(&content, id).external_changed);
        assert_eq!(
            content.model().query_tab_sql(id).as_deref(),
            Some(later.as_str())
        );
        edit(&buffer, "-- after external change\n");
        glib::MainContext::default().block_on(glib::timeout_future(Duration::from_millis(50)));
        assert!(doc(&content, id).external_changed);
        buffer.undo();
        content.emit(WindowContentMsg::Document(DocumentMsg::ReloadTab(Some(
            format!("query-tab-{id}"),
        ))));
        until(|| content.model().query_tab_sql(id).as_deref() == Some(external));
        assert!(!buffer.can_undo());
        assert!(!doc(&content, id).external_changed);

        edit(&buffer, "-- saved after reload\n");
        let final_text = content.model().query_tab_sql(id).unwrap();
        content.emit(WindowContentMsg::SaveSqlFile);
        until(|| doc(&content, id).saved_text == final_text);
        assert_eq!(fs::read(&path).unwrap(), bytes(&final_text, false));
        content.emit(WindowContentMsg::CloseQueryTab(id));
        until(|| content.model().query_tabs.iter().all(|tab| tab.id != id));
        assert_eq!(fs::read(&path).unwrap(), bytes(&final_text, false));
        root.destroy();
        drop(content);
        glib::MainContext::default().block_on(glib::timeout_future(Duration::from_millis(600)));
    }

    #[test]
    #[ignore = "requires a display and isolated XDG directories; run in its own process"]
    fn gtk_deferred_database_navigation() {
        use crate::models::connection::ConnectionForm;
        use crate::state::session_store;
        use std::time::{Duration, Instant};

        #[track_caller]
        fn until(mut ready: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready() {
                assert!(
                    Instant::now() < deadline,
                    "document navigation did not settle"
                );
                while glib::MainContext::default().pending() {
                    glib::MainContext::default().iteration(false);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _runtime = runtime.enter();
        let _app = relm4::RelmApp::<WindowContentMsg>::new(crate::config::APP_ID);
        sourceview5::init();
        crate::register_app_resources();
        let application = relm4::main_adw_application();
        application.register(gio::Cancellable::NONE).unwrap();
        let content = WindowContent::builder().launch(()).detach();
        let root = adw::ApplicationWindow::builder()
            .application(&application)
            .content(content.widget())
            .build();
        root.present();
        let details = ConnectionForm {
            name: "Navigation Test".into(),
            username: "test".into(),
            database: "before".into(),
            ..ConnectionForm::default()
        }
        .validate()
        .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://test@127.0.0.1:1/unused")
            .unwrap();
        {
            let mut parts = content.state().get_mut();
            let ComponentParts { model, widgets } = &mut *parts;
            model.visible_page = super::super::VisiblePage::Workspace;
            model.state.active_connection = Some(details.saved.clone());
            model.state.active_database = Some("before".into());
            model.active_connection_details = Some(details.clone());
            model.active_pool = Some(pool.clone());
            model.active_database_switch_request_id = Some(7);
            model.query_tabs[0]
                .editor_buffer
                .set_text("before connecting");
            model.save_current_session(widgets).unwrap();
            model.query_tabs[0]
                .editor_buffer
                .set_text("latest edits while connecting");
            model.documents.chooser_open = true;
        }
        content.emit(WindowContentMsg::DatabaseSwitchCompleted {
            id: 7,
            database: "after".into(),
            result: Ok(PostgresConnection {
                pool: pool.clone(),
                tunnel: None,
            }),
        });
        until(|| content.model().documents.deferred_switch.is_some());
        assert_eq!(
            content.model().state.active_database.as_deref(),
            Some("before")
        );
        content.state().get_mut().model.documents.chooser_open = false;
        content.emit(WindowContentMsg::Document(DocumentMsg::SyncFileActions));
        until(|| {
            let model = content.model();
            model.state.active_database.as_deref() == Some("after")
                && !model.file_operation_pending()
        });
        let old_session = session_store::load(&details.saved.id, "before").unwrap();
        assert!(
            matches!(&old_session.tabs[0], SavedSessionTab::Query { sql, .. } if sql == "latest edits while connecting")
        );

        let scratch_count = content.model().query_tabs.len();
        let large_scratch = "x".repeat(300 * 1024);
        content.state().get_mut().model.query_tabs[0]
            .editor_buffer
            .set_text(&large_scratch);
        content.emit(WindowContentMsg::ShowStartScreen);
        content.emit(WindowContentMsg::NewQueryTab);
        until(|| content.model().query_tabs.len() == scratch_count + 1);
        assert!(content.model().shows_workspace());
        assert_eq!(
            content
                .model()
                .query_tab_sql(content.model().query_tabs[0].id)
                .as_deref(),
            Some(large_scratch.as_str())
        );
        content.state().get_mut().model.query_tabs[0]
            .editor_buffer
            .set_text("small scratch");

        let (_, record) = content
            .state()
            .get_mut()
            .model
            .install_protocol_test_document();
        content.emit(WindowContentMsg::PrepareClose);
        until(|| content.model().close_preparation_pending());
        {
            let mut parts = content.state().get_mut();
            parts.model.documents.disconnect_pending = true;
            parts.model.documents.deferred_connection =
                Some((details, PostgresConnection { pool, tunnel: None }));
        }
        content.emit(WindowContentMsg::CancelClosePreparation);
        until(|| {
            let model = content.model();
            model.shows_start_screen() && !model.close_preparation_pending()
        });
        assert!(content.model().documents.deferred_connection.is_none());
        assert!(content.model().documents.deferred_switch.is_none());
        assert!(content.model().state.active_connection.is_none());
        assert!(content.widget().is_sensitive());
        assert!(content.model().query_tabs.iter().any(|tab| {
            content.model().query_tab_sql(tab.id).as_deref() == Some(record.text.as_str())
        }));
        document_store::remove(&record.document_id).unwrap();
        root.destroy();
        drop(content);
        let debounce_deadline = Instant::now() + Duration::from_millis(600);
        until(|| Instant::now() >= debounce_deadline);
    }

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
    fn close_decisions_are_serial_and_cannot_mutate_target_set() {
        let targets = vec!["query-tab-2".into(), "query-tab-3".into()];
        let mut close = ClosePreparation {
            targets: Some(targets.clone()),
            remaining: vec![2, 3],
            discarded: Vec::new(),
            approved: false,
            generation: 19,
            committing: false,
            cancelling: false,
            committed: false,
        };
        assert!(!close.accepts_decision(18, 3));
        assert!(!close.accepts_decision(19, 2));
        assert!(close.accepts_decision(19, 3));
        close.discarded.push(close.remaining.pop().unwrap());
        assert!(!close.accepts_decision(19, 3));
        assert!(close.accepts_decision(19, 2));
        assert_eq!(close.targets.as_ref(), Some(&targets));
        assert!(!close.approved);
        close.approved = true;
        assert!(!close.accepts_decision(19, 2));
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
    fn cancellation_invalidates_cleanup_and_late_save_decisions_even_after_commit() {
        for committed in [false, true] {
            let targets = vec!["query-tab-5".into()];
            let mut close = ClosePreparation {
                targets: Some(targets.clone()),
                remaining: vec![5],
                discarded: Vec::new(),
                approved: true,
                generation: 20,
                committing: true,
                cancelling: false,
                committed,
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
            discarded: Vec::new(),
            approved: false,
            generation: 20,
            committing: false,
            cancelling: false,
            committed: false,
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

    fn session_with_promoted_source() -> SavedSession {
        use crate::models::session::{SavedSessionObject, SavedSessionObjectKind};

        SavedSession {
            connection_id: "original-connection".into(),
            database: "original-database".into(),
            active_tab: Some(SavedSessionTabId::Query(42)),
            tabs: vec![
                SavedSessionTab::Query {
                    id: 42,
                    sql: "same SQL".into(),
                    row_limit: 1000,
                },
                SavedSessionTab::Query {
                    id: 43,
                    sql: "same SQL".into(),
                    row_limit: 1000,
                },
                SavedSessionTab::Browse {
                    id: 42,
                    object: SavedSessionObject {
                        schema: "public".into(),
                        name: "items".into(),
                        kind: SavedSessionObjectKind::Table,
                    },
                },
            ],
        }
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

    #[test]
    fn promotion_suppression_removes_only_the_original_query_in_its_original_scope() {
        let mut doc = document();
        doc.source_tab_id = Some(42);
        doc.saved_text = "same SQL".into();
        let record = doc.recovery_record("recovered text".into());
        let scope = doc.origin.clone();
        let records = [record];
        let mut session = session_with_promoted_source();
        suppress_promoted_scratch_tabs(&mut session, &scope, &records);
        assert_eq!(
            session
                .tabs
                .iter()
                .map(SavedSessionTab::id)
                .collect::<Vec<_>>(),
            vec![SavedSessionTabId::Query(43), SavedSessionTabId::Browse(42)]
        );
        assert_eq!(session.active_tab, None);

        for other_scope in [
            ("other-connection".into(), scope.1.clone()),
            (scope.0.clone(), "other-database".into()),
        ] {
            let mut session = session_with_promoted_source();
            let original = session.clone();
            suppress_promoted_scratch_tabs(&mut session, &other_scope, &records);
            assert_eq!(session, original);
        }
    }

    #[test]
    fn recovery_report_warnings_do_not_block_healthy_promotion_or_suppress_unrelated_tabs() {
        let mut doc = document();
        doc.source_tab_id = Some(42);
        doc.saved_text = "same SQL".into();
        let healthy = doc.recovery_record("recovered text".into());
        let mut other_scope = healthy.clone();
        other_scope.database = "other-database".into();
        other_scope.source_tab_id = Some(43);
        let report = RecoveryLoad {
            records: vec![healthy, other_scope],
            warnings: vec![
                "broken-a.json: invalid JSON".into(),
                "broken-b.json: unreadable".into(),
            ],
        };
        let sources = promotion_sources(&report.records, &doc.origin);
        assert_eq!(sources, HashSet::from([42]));
        let mut session = session_with_promoted_source();
        session.active_tab = Some(SavedSessionTabId::Browse(42));
        suppress_promoted_scratch_tabs(&mut session, &doc.origin, &report.records);
        assert_eq!(session.active_tab, Some(SavedSessionTabId::Browse(42)));
        assert!(
            session
                .tabs
                .iter()
                .any(|tab| tab.id() == SavedSessionTabId::Query(43))
        );
        let body = recovery_warning_body(&report.warnings);
        assert!(body.contains("2 recovery files"));
        assert!(body.contains("broken-a.json"));
        assert!(body.contains("broken-b.json"));
    }

    #[test]
    fn legacy_recovery_without_provenance_never_guesses_a_duplicate_from_sql_text() {
        let doc = document();
        let record = doc.recovery_record("same SQL".into());
        let records = [record];
        let mut session = session_with_promoted_source();
        let original = session.clone();
        suppress_promoted_scratch_tabs(&mut session, &doc.origin, &records);
        assert_eq!(session, original);
    }

    #[test]
    fn promotion_never_suppresses_newer_edits_from_another_window_with_the_same_source_id() {
        let mut doc = document();
        doc.source_tab_id = Some(42);
        doc.saved_text = "same SQL".into();
        let records = [doc.recovery_record("recovered file edits".into())];
        let mut session = session_with_promoted_source();
        if let SavedSessionTab::Query { sql, .. } = &mut session.tabs[0] {
            *sql = "newer independent scratch edits".into();
        }
        let original = session.clone();
        suppress_promoted_scratch_tabs(&mut session, &doc.origin, &records);
        assert_eq!(session, original);
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
