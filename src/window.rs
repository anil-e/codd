use std::cell::RefCell;
use std::collections::HashMap;

use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::Sender;
use relm4::gtk::glib::variant::ToVariant;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;

use crate::app_window::{
    AppWindow as ExtraAppWindow, AppWindowInit, AppWindowMsg as ExtraAppWindowMsg, AppWindowOutput,
};
use crate::config::{APP_ID, RESOURCE_PREFIX};
use crate::settings;
use crate::window_actions::{self, WindowAction};
use crate::window_content::{WindowContent, WindowContentMsg, WindowContentOutput};

thread_local! {
    static OPEN_FILES: RefCell<(Option<Sender<AppWindowMsg>>, Vec<gio::File>)> = const {
        RefCell::new((None, Vec::new()))
    };
}

pub fn open_sql_files(files: Vec<gio::File>) {
    OPEN_FILES.with(|routing| {
        let mut routing = routing.borrow_mut();
        if let Some(sender) = &routing.0 {
            let _ = sender.send(AppWindowMsg::OpenSqlFileLocations(files));
        } else {
            routing.1.extend(files);
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClosePhase {
    Preparing,
    Prepared,
    Committing,
    Committed,
    Cancelling,
}

#[derive(Default)]
struct CloseState {
    windows: HashMap<u64, ClosePhase>,
    quitting: bool,
    quit_requested: bool,
    primary_generation: u64,
}

impl CloseState {
    fn request(&mut self, id: u64) -> bool {
        if self.quitting
            || self.quit_requested
            || self.windows.contains_key(&id)
            || self
                .windows
                .values()
                .any(|phase| *phase == ClosePhase::Cancelling)
        {
            return false;
        }
        self.windows.insert(id, ClosePhase::Preparing);
        true
    }

    fn begin_quit(&mut self, ids: impl IntoIterator<Item = u64>) -> Option<Vec<u64>> {
        if self.quitting {
            return None;
        }
        self.quit_requested = true;
        if self.windows.values().any(|phase| {
            matches!(
                phase,
                ClosePhase::Committing | ClosePhase::Committed | ClosePhase::Cancelling
            )
        }) {
            return None;
        }
        self.quitting = true;
        let mut prepare = Vec::new();
        for id in ids {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.windows.entry(id) {
                entry.insert(ClosePhase::Preparing);
                prepare.push(id);
            }
        }
        Some(prepare)
    }

    fn accepts(&self, id: u64, generation: u64) -> bool {
        id != 0 || generation == self.primary_generation
    }

    fn prepared(&mut self, id: u64, generation: u64) -> Vec<u64> {
        if !self.accepts(id, generation) || self.windows.get(&id) != Some(&ClosePhase::Preparing) {
            return Vec::new();
        }
        self.windows.insert(id, ClosePhase::Prepared);
        let commit = if !self.quitting {
            vec![id]
        } else if self
            .windows
            .values()
            .all(|phase| *phase == ClosePhase::Prepared)
        {
            self.windows.keys().copied().collect()
        } else {
            Vec::new()
        };
        for id in &commit {
            self.windows.insert(*id, ClosePhase::Committing);
        }
        commit
    }

    fn cancelled(&mut self, id: u64, generation: u64) -> Option<Vec<u64>> {
        if !self.accepts(id, generation) || self.windows.remove(&id).is_none() {
            return None;
        }
        let mut cancel = Vec::new();
        if self.quitting {
            self.quitting = false;
            self.quit_requested = false;
            cancel.push(id);
            for (&other, phase) in &mut self.windows {
                *phase = ClosePhase::Cancelling;
                cancel.push(other);
            }
        }
        Some(cancel)
    }

    fn committed(&mut self, id: u64, generation: u64) -> bool {
        if !self.accepts(id, generation) || self.windows.get(&id) != Some(&ClosePhase::Committing) {
            return false;
        }
        self.windows.insert(id, ClosePhase::Committed);
        true
    }

    fn all_committed(&self) -> bool {
        self.quitting
            && !self.windows.is_empty()
            && self
                .windows
                .values()
                .all(|phase| *phase == ClosePhase::Committed)
    }
}

pub struct AppWindow {
    content: Controller<WindowContent>,
    windows: HashMap<u64, Controller<ExtraAppWindow>>,
    next_window_id: u64,
    close: CloseState,
    primary_hidden: bool,
}

#[derive(Debug)]
pub enum AppWindowMsg {
    OpenSqlFiles,
    OpenSqlFileLocations(Vec<gio::File>),
    SaveSqlFile,
    SaveSqlFileAs,
    RequestClose(u64),
    ClosePrepared(u64, u64),
    CloseCancelled(u64, u64),
    CloseCommitted(u64, u64),
    PrimaryPresented,
    OpenConnectionDialog,
    NewQueryTab,
    CloseActiveTab,
    RunQuery,
    CancelQuery,
    RefreshTableBrowser,
    RefreshWorkspace,
    FocusEditor,
    FocusObjectSearch,
    NewWindow,
    ExtraWindowClosed(u64),
    Shortcuts,
    About,
    Quit,
}

#[relm4::component(pub)]
impl Component for AppWindow {
    type Init = ();
    type Input = AppWindowMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        adw::ApplicationWindow {
            set_title: Some("Codd"),
            set_default_size: (1180, 760),
            set_width_request: 360,
            set_height_request: 320,

            #[wrap(Some)]
            set_content = model.content.widget(),
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        sourceview5::init();

        let app_settings = settings::app_settings();
        let content = launch_content(&sender, 0);
        let model = AppWindow {
            content,
            windows: HashMap::new(),
            next_window_id: 1,
            close: CloseState::default(),
            primary_hidden: false,
        };
        let widgets = view_output!();

        let width = app_settings.int("window-width");
        let height = app_settings.int("window-height");
        root.set_default_width(width);
        root.set_default_height(height);
        root.connect_close_request({
            let sender = sender.clone();
            move |window| {
                let settings = settings::app_settings();
                let _ = settings.set_int("window-width", window.width());
                let _ = settings.set_int("window-height", window.height());
                sender.input(AppWindowMsg::RequestClose(0));
                glib::Propagation::Stop
            }
        });

        root.connect_map({
            let sender = sender.clone();
            move |_| sender.input(AppWindowMsg::PrimaryPresented)
        });

        relm4::main_adw_application().set_resource_base_path(Some(RESOURCE_PREFIX));

        window_actions::setup_window_actions(&root, {
            let sender = sender.clone();
            move |action| sender.input(AppWindowMsg::from(action))
        });
        setup_app_actions(&sender);

        OPEN_FILES.with(|routing| {
            let mut routing = routing.borrow_mut();
            routing.0 = Some(sender.input_sender().clone());
            if !routing.1.is_empty() {
                sender.input(AppWindowMsg::OpenSqlFileLocations(std::mem::take(
                    &mut routing.1,
                )));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            AppWindowMsg::OpenSqlFiles if !self.close.quitting && !self.close.quit_requested => {
                self.reopen_primary(root);
                self.content.emit(WindowContentMsg::OpenSqlFiles);
            }
            AppWindowMsg::OpenSqlFileLocations(files)
                if !self.close.quitting && !self.close.quit_requested =>
            {
                let active = relm4::main_adw_application().active_window();
                if let Some(window) = self.windows.values().find(|window| {
                    active
                        .as_ref()
                        .is_some_and(|active| active == window.widget())
                }) {
                    window.emit(ExtraAppWindowMsg::OpenSqlFileLocations(files));
                } else {
                    self.reopen_primary(root);
                    root.present();
                    self.content
                        .emit(WindowContentMsg::OpenSqlFileLocations(files));
                }
            }
            AppWindowMsg::OpenSqlFiles | AppWindowMsg::OpenSqlFileLocations(_) => {}
            AppWindowMsg::SaveSqlFile if !self.close.quitting && !self.close.quit_requested => {
                self.content.emit(WindowContentMsg::SaveSqlFile)
            }
            AppWindowMsg::SaveSqlFileAs if !self.close.quitting && !self.close.quit_requested => {
                self.content.emit(WindowContentMsg::SaveSqlFileAs)
            }
            AppWindowMsg::SaveSqlFile | AppWindowMsg::SaveSqlFileAs => {}
            AppWindowMsg::PrimaryPresented => {
                if self.primary_hidden {
                    if self.close.quitting || self.close.quit_requested {
                        root.set_visible(false);
                    } else {
                        self.reopen_primary(root);
                    }
                }
            }
            AppWindowMsg::RequestClose(id) => {
                if (id != 0 || !self.primary_hidden)
                    && (id == 0 || self.windows.contains_key(&id))
                    && self.close.request(id)
                {
                    self.prepare_close(id);
                }
            }
            AppWindowMsg::ClosePrepared(id, generation) => {
                for id in self.close.prepared(id, generation) {
                    self.commit_close(id);
                }
            }
            AppWindowMsg::CloseCancelled(id, generation) => {
                if let Some(cancel) = self.close.cancelled(id, generation) {
                    if !self.close.quit_requested {
                        for window in self.windows.values() {
                            window.emit(ExtraAppWindowMsg::SetQuitting(false));
                        }
                    }
                    for id in cancel {
                        if id == 0 {
                            self.content.emit(WindowContentMsg::CancelClosePreparation);
                        } else if let Some(window) = self.windows.get(&id) {
                            window.emit(ExtraAppWindowMsg::CancelClosePreparation);
                        }
                    }
                    self.resume_quit(&sender);
                }
            }
            AppWindowMsg::CloseCommitted(id, generation) => {
                if self.close.committed(id, generation) {
                    if self.close.quitting {
                        if self.close.all_committed() {
                            relm4::main_adw_application().quit();
                        }
                    } else if id == 0 {
                        self.close.windows.remove(&id);
                        if self.windows.is_empty() {
                            relm4::main_adw_application().quit();
                        } else {
                            self.primary_hidden = true;
                            root.set_visible(false);
                            self.close.primary_generation += 1;
                            self.content = launch_content(&sender, self.close.primary_generation);
                            root.set_content(Some(self.content.widget()));
                            window_actions::set_file_actions_enabled(root, false, false);
                            self.resume_quit(&sender);
                        }
                    } else if let Some(window) = self.windows.get(&id) {
                        window.emit(ExtraAppWindowMsg::FinishClose);
                    }
                }
            }
            AppWindowMsg::OpenConnectionDialog => {
                self.content.emit(WindowContentMsg::OpenConnectionDialog);
            }
            AppWindowMsg::NewQueryTab => {
                self.content.emit(WindowContentMsg::NewQueryTab);
            }
            AppWindowMsg::CloseActiveTab => {
                self.content.emit(WindowContentMsg::CloseActiveTab);
            }
            AppWindowMsg::RunQuery => {
                self.content.emit(WindowContentMsg::RunQuery);
            }
            AppWindowMsg::CancelQuery => {
                self.content.emit(WindowContentMsg::CancelQuery);
            }
            AppWindowMsg::RefreshTableBrowser => {
                self.content.emit(WindowContentMsg::RefreshActiveBrowseTab);
            }
            AppWindowMsg::RefreshWorkspace => {
                self.content.emit(WindowContentMsg::RefreshWorkspace);
            }
            AppWindowMsg::FocusEditor => {
                self.content.emit(WindowContentMsg::FocusEditor);
            }
            AppWindowMsg::FocusObjectSearch => {
                self.content.emit(WindowContentMsg::FocusObjectSearch);
            }
            AppWindowMsg::NewWindow => {
                if !self.close.quitting && !self.close.quit_requested {
                    if self.primary_hidden && self.windows.is_empty() {
                        self.reopen_primary(root);
                    } else {
                        self.spawn_window(&sender);
                    }
                }
            }
            AppWindowMsg::ExtraWindowClosed(id) => {
                self.windows.remove(&id);
                self.close.windows.remove(&id);
                if self.windows.is_empty() && self.primary_hidden {
                    relm4::main_adw_application().quit();
                } else {
                    self.resume_quit(&sender);
                }
            }
            AppWindowMsg::Shortcuts => {
                show_shortcuts_dialog(root);
            }
            AppWindowMsg::About => {
                show_about_dialog(root);
            }
            AppWindowMsg::Quit => {
                if self.close.quitting {
                    return;
                }
                for window in self.windows.values() {
                    window.emit(ExtraAppWindowMsg::SetQuitting(true));
                }
                let mut ids: Vec<_> = self.windows.keys().copied().collect();
                if !self.primary_hidden {
                    ids.push(0);
                }
                if let Some(prepare) = self.close.begin_quit(ids) {
                    if self.close.windows.is_empty() {
                        relm4::main_adw_application().quit();
                    }
                    for id in prepare {
                        self.prepare_close(id);
                    }
                }
            }
        }
    }
}

fn setup_app_actions(sender: &ComponentSender<AppWindow>) {
    let app = relm4::main_adw_application();
    let app_settings = settings::app_settings();

    let action = gtk::gio::SimpleAction::new_stateful(
        "compact-mode",
        None,
        &app_settings.boolean("compact-mode").to_variant(),
    );

    action.connect_activate({
        let app_settings = app_settings.clone();
        move |action, _| {
            let compact_mode = !app_settings.boolean("compact-mode");
            let _ = app_settings.set_boolean("compact-mode", compact_mode);
            action.set_state(&compact_mode.to_variant());
        }
    });

    action.connect_change_state({
        let app_settings = app_settings.clone();
        move |action, value| {
            let Some(compact_mode) = value.and_then(|value| value.get::<bool>()) else {
                return;
            };

            let _ = app_settings.set_boolean("compact-mode", compact_mode);
            action.set_state(&compact_mode.to_variant());
        }
    });

    app_settings.connect_changed(Some("compact-mode"), {
        let action = action.clone();
        move |settings, _| {
            action.set_state(&settings.boolean("compact-mode").to_variant());
        }
    });

    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("new-window", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::NewWindow);
    });
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("shortcuts", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::Shortcuts);
    });
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("about", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::About);
    });
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("quit", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::Quit);
    });
    app.add_action(&action);

    app.set_accels_for_action("win.new-query-tab", &["<Control>n"]);
    app.set_accels_for_action("win.open-sql-file", &["<Control>o"]);
    app.set_accels_for_action("win.save-sql-file", &["<Control>s"]);
    app.set_accels_for_action("win.save-sql-file-as", &["<Control><Shift>s"]);
    app.set_accels_for_action("win.close-active-tab", &["<Control>w"]);
    app.set_accels_for_action("app.new-window", &["<Control><Shift>n"]);
    app.set_accels_for_action("win.run-query", &["<Control>Return"]);
    app.set_accels_for_action("win.cancel-query", &["Escape"]);
    app.set_accels_for_action("win.refresh-table-browser", &["<Control>r"]);
    app.set_accels_for_action("win.refresh-workspace", &["F5"]);
    app.set_accels_for_action("win.focus-editor", &["<Control>e"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("app.shortcuts", &["<Control>question"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}

fn show_shortcuts_dialog(root: &adw::ApplicationWindow) {
    let resource_path = format!("{RESOURCE_PREFIX}/ui/shortcuts-dialog.ui");
    let builder = gtk::Builder::from_resource(&resource_path);
    let dialog: adw::ShortcutsDialog = builder
        .object("shortcuts_dialog")
        .expect("shortcuts dialog to exist");

    let app = relm4::main_adw_application();
    if let Some(window) = app.active_window() {
        dialog.present(Some(&window));
    } else {
        dialog.present(Some(root));
    }
}

fn show_about_dialog(root: &adw::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("Codd")
        .application_icon(APP_ID)
        .developer_name("Anil Erdogan")
        .version(env!("CARGO_PKG_VERSION"))
        .developers(vec!["Anil Erdogan"])
        .translator_credits(gettext("translator-credits"))
        .copyright("© 2026 Anil Erdogan")
        .comments(gettext("Lightweight PostgreSQL client"))
        .website("https://github.com/anil-e/codd")
        .issue_url("https://github.com/anil-e/codd/issues")
        .license_type(gtk::License::Agpl30)
        .build();

    let app = relm4::main_adw_application();
    if let Some(window) = app.active_window() {
        about.present(Some(&window));
    } else {
        about.present(Some(root));
    }
}

impl From<WindowAction> for AppWindowMsg {
    fn from(action: WindowAction) -> Self {
        match action {
            WindowAction::OpenSqlFiles => Self::OpenSqlFiles,
            WindowAction::SaveSqlFile => Self::SaveSqlFile,
            WindowAction::SaveSqlFileAs => Self::SaveSqlFileAs,
            WindowAction::OpenConnectionDialog => Self::OpenConnectionDialog,
            WindowAction::NewQueryTab => Self::NewQueryTab,
            WindowAction::CloseActiveTab => Self::CloseActiveTab,
            WindowAction::RunQuery => Self::RunQuery,
            WindowAction::CancelQuery => Self::CancelQuery,
            WindowAction::RefreshTableBrowser => Self::RefreshTableBrowser,
            WindowAction::RefreshWorkspace => Self::RefreshWorkspace,
            WindowAction::FocusEditor => Self::FocusEditor,
            WindowAction::FocusObjectSearch => Self::FocusObjectSearch,
        }
    }
}

impl AppWindow {
    fn prepare_close(&self, id: u64) {
        if id == 0 {
            self.content.emit(WindowContentMsg::PrepareClose);
        } else if let Some(window) = self.windows.get(&id) {
            window.emit(ExtraAppWindowMsg::PrepareClose);
        }
    }

    fn commit_close(&self, id: u64) {
        if id == 0 {
            self.content.emit(WindowContentMsg::CommitClose);
        } else if let Some(window) = self.windows.get(&id) {
            window.emit(ExtraAppWindowMsg::CommitClose);
        }
    }

    fn resume_quit(&self, sender: &ComponentSender<Self>) {
        if self.close.quit_requested && !self.close.quitting && self.close.windows.is_empty() {
            sender.input(AppWindowMsg::Quit);
        }
    }

    fn reopen_primary(&mut self, root: &adw::ApplicationWindow) {
        if self.primary_hidden {
            self.primary_hidden = false;
            root.present();
        }
    }

    fn spawn_window(&mut self, sender: &ComponentSender<Self>) {
        let id = self.next_window_id;
        self.next_window_id = self.next_window_id.wrapping_add(1);

        let window = ExtraAppWindow::builder()
            .launch(AppWindowInit { id })
            .forward(sender.input_sender(), |output| match output {
                AppWindowOutput::CloseRequested(id) => AppWindowMsg::RequestClose(id),
                AppWindowOutput::ClosePrepared(id) => AppWindowMsg::ClosePrepared(id, 0),
                AppWindowOutput::CloseCancelled(id) => AppWindowMsg::CloseCancelled(id, 0),
                AppWindowOutput::CloseCommitted(id) => AppWindowMsg::CloseCommitted(id, 0),
                AppWindowOutput::Closed(id) => AppWindowMsg::ExtraWindowClosed(id),
            });

        self.windows.insert(id, window);
    }
}

fn launch_content(
    sender: &ComponentSender<AppWindow>,
    generation: u64,
) -> Controller<WindowContent> {
    WindowContent::builder()
        .launch(())
        .forward(sender.input_sender(), move |output| match output {
            WindowContentOutput::ClosePrepared => AppWindowMsg::ClosePrepared(0, generation),
            WindowContentOutput::CloseCancelled => AppWindowMsg::CloseCancelled(0, generation),
            WindowContentOutput::CloseCommitted => AppWindowMsg::CloseCommitted(0, generation),
        })
}

#[cfg(test)]
mod tests {
    use super::{ClosePhase, CloseState};

    #[test]
    fn quit_waits_for_every_prepare_and_every_commit() {
        let mut close = CloseState::default();
        assert_eq!(close.begin_quit([0, 1, 2]), Some(vec![0, 1, 2]));
        assert!(close.prepared(2, 0).is_empty());
        assert!(close.prepared(0, 0).is_empty());
        assert!(!close.committed(0, 0));
        assert!(!close.all_committed());

        let mut commit = close.prepared(1, 0);
        commit.sort_unstable();
        assert_eq!(commit, vec![0, 1, 2]);
        assert!(close.committed(1, 0));
        assert!(close.committed(2, 0));
        assert!(!close.all_committed());
        assert!(close.committed(0, 0));
        assert!(close.all_committed());
    }

    #[test]
    fn prepare_cancellation_waits_for_all_restore_acknowledgements() {
        let mut close = CloseState::default();
        close.begin_quit([0, 1, 2]);
        assert!(close.prepared(0, 0).is_empty());

        let mut cancel = close.cancelled(1, 0).unwrap();
        cancel.sort_unstable();
        assert_eq!(cancel, vec![0, 1, 2]);
        assert!(!close.quitting);
        assert!(!close.quit_requested);
        assert_eq!(close.windows.len(), 2);
        assert!(
            close
                .windows
                .values()
                .all(|phase| *phase == ClosePhase::Cancelling)
        );
        assert!(close.prepared(2, 0).is_empty());
        assert!(!close.committed(0, 0));
        assert!(!close.request(1));

        assert_eq!(close.cancelled(0, 0), Some(vec![]));
        assert!(!close.request(1));
        assert_eq!(close.cancelled(2, 0), Some(vec![]));
        assert!(close.windows.is_empty());
        assert!(close.request(1));
    }

    #[test]
    fn cleanup_failure_rolls_back_committed_and_in_flight_windows() {
        let mut close = CloseState::default();
        close.begin_quit([0, 1, 2]);
        close.prepared(0, 0);
        close.prepared(1, 0);
        assert_eq!(close.prepared(2, 0).len(), 3);
        assert!(close.committed(0, 0));

        assert_eq!(close.cancelled(1, 0).unwrap().len(), 3);
        assert!(!close.all_committed());
        assert!(!close.committed(2, 0));
        assert!(!close.committed(0, 0));
        assert_eq!(close.begin_quit([0, 1, 2]), None);
        assert!(!close.quitting);
        close.cancelled(0, 0);
        close.cancelled(2, 0);
        assert!(close.windows.is_empty());
        assert_eq!(close.begin_quit([0, 1, 2]), Some(vec![0, 1, 2]));
    }

    #[test]
    fn quit_adopts_existing_individual_preparation() {
        let mut close = CloseState::default();
        assert!(close.request(1));
        assert_eq!(close.begin_quit([0, 1]), Some(vec![0]));
        assert!(close.prepared(1, 0).is_empty());
        assert_eq!(close.prepared(0, 0).len(), 2);
        assert!(!close.request(2));
        assert_eq!(close.begin_quit([0, 1]), None);
    }

    #[test]
    fn quit_defers_until_individual_close_is_finished() {
        let mut close = CloseState::default();
        assert!(close.request(1));
        assert_eq!(close.prepared(1, 0), vec![1]);
        assert_eq!(close.begin_quit([0, 1]), None);
        assert!(close.quit_requested);
        assert!(!close.quitting);
        assert!(!close.request(0));
        assert!(close.committed(1, 0));
        assert!(!close.all_committed());
        assert_eq!(close.begin_quit([0, 1]), None);

        close.windows.remove(&1);
        assert_eq!(close.begin_quit([0]), Some(vec![0]));
        assert_eq!(close.prepared(0, 0), vec![0]);
        assert!(close.committed(0, 0));
        assert!(close.all_committed());
    }

    #[test]
    fn duplicate_and_unknown_close_outputs_do_not_advance_state() {
        let mut close = CloseState::default();
        assert!(close.request(1));
        assert!(!close.request(1));
        assert!(close.prepared(9, 0).is_empty());
        assert!(!close.committed(1, 0));
        assert_eq!(close.cancelled(9, 0), None);
        assert_eq!(close.prepared(1, 0), vec![1]);
        assert!(close.prepared(1, 0).is_empty());
        assert!(close.committed(1, 0));
        assert!(!close.committed(1, 0));
    }

    #[test]
    fn replaced_primary_rejects_all_outputs_from_old_content() {
        let mut close = CloseState::default();
        assert!(close.request(0));
        assert_eq!(close.prepared(0, 0), vec![0]);
        assert!(close.committed(0, 0));
        close.windows.remove(&0);
        close.primary_generation += 1;

        assert!(close.request(0));
        assert!(close.prepared(0, 0).is_empty());
        assert_eq!(close.cancelled(0, 0), None);
        assert!(!close.committed(0, 0));
        assert_eq!(close.windows.get(&0), Some(&ClosePhase::Preparing));
        assert_eq!(close.prepared(0, 1), vec![0]);
        assert_eq!(close.cancelled(0, 0), None);
        assert!(!close.committed(0, 0));
        assert!(close.committed(0, 1));
    }

    #[test]
    fn hidden_primary_is_not_a_quit_participant() {
        let mut close = CloseState::default();
        assert_eq!(close.begin_quit([1, 2]), Some(vec![1, 2]));
        assert!(close.prepared(0, 0).is_empty());
        assert!(close.prepared(1, 0).is_empty());
        assert_eq!(close.prepared(2, 0).len(), 2);
        assert!(close.committed(1, 0));
        assert!(!close.all_committed());
        assert!(close.committed(2, 0));
        assert!(close.all_committed());
    }
}
