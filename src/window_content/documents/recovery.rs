use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::Duration;

use relm4::Sender;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::prelude::*;

use super::{DocumentMsg, PendingSession, WindowContent, WindowContentMsg, WindowContentWidgets};
use crate::models::session::{SavedSession, SavedSessionTab};
use crate::state::document_store::{self, RecoveryRecord};
use crate::state::session_store;
use crate::window_content::closing::ClosingMsg;

pub(super) fn promotion_sources(
    records: &[RecoveryRecord],
    scope: &(String, String),
) -> HashSet<u64> {
    records
        .iter()
        .filter(|record| record.connection_id == scope.0 && record.database == scope.1)
        .filter_map(|record| record.source_tab_id)
        .collect()
}

pub(super) fn suppress_promoted_scratch_tabs(
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
                            let _ = sender.send(WindowContentMsg::Closing(
                                ClosingMsg::CleanupFinished { generation, result },
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
                        let _ = sender.send(WindowContentMsg::Closing(
                            ClosingMsg::CancellationFinished { generation, result },
                        ));
                    }
                }
            }
        });
        tx
    })
}

impl WindowContent {
    pub(in crate::window_content) fn document_text_changed(
        &mut self,
        id: u64,
        sender: &ComponentSender<Self>,
    ) {
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

    pub(in crate::window_content) fn persist_document(
        &self,
        id: u64,
        sender: &ComponentSender<Self>,
    ) {
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

    pub(in crate::window_content) fn restore_documents(
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

    pub(in crate::window_content) fn restore_close_recovery(
        &self,
        generation: u64,
        sender: &ComponentSender<Self>,
    ) {
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

    pub(in crate::window_content) fn save_close_session(
        &self,
        targets: Option<&[String]>,
        widgets: &WindowContentWidgets,
    ) -> Result<(), String> {
        let whole_window = targets.is_none();
        let records: Vec<_> = self
            .query_tabs
            .iter()
            .filter(|tab| {
                targets.is_none_or(|targets| targets.contains(&format!("query-tab-{}", tab.id)))
            })
            .filter_map(|tab| {
                let doc = tab.document.as_ref()?;
                doc.source_tab_id
                    .map(|_| doc.recovery_record(self.query_tab_sql(tab.id).unwrap_or_default()))
            })
            .collect();
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

        Ok(())
    }

    pub(in crate::window_content) fn remove_close_recovery(
        &self,
        targets: Option<&[String]>,
        generation: u64,
        sender: &ComponentSender<Self>,
    ) {
        let ids = self
            .query_tabs
            .iter()
            .filter(|tab| {
                targets.is_none_or(|names| names.contains(&format!("query-tab-{}", tab.id)))
            })
            .filter_map(|tab| tab.document.as_ref().map(|doc| doc.id.clone()))
            .collect();
        let _ = recovery_worker().send(RecoveryJob::Remove(
            ids,
            Some((generation, sender.input_sender().clone())),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::document;
    use super::*;
    use crate::models::session::SavedSessionTabId;
    use crate::state::document_store::RecoveryLoad;

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
}
