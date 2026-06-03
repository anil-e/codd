use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;

use relm4::Sender;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;
use sqlx::PgPool;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use crate::db;
use crate::menus;
use crate::models::connection::{ConnectionDetails, SavedConnection};
use crate::models::csv_export::{self, CsvExportOptions};
use crate::models::database_object::DatabaseObject;
use crate::models::query_history::QueryHistoryEntry;
use crate::models::query_result::QueryExecutionResult;
use crate::models::session::SavedSession;
use crate::models::table_script::TableScriptKind;
use crate::settings;
use crate::state::{app_state::AppState, connection_store, credential_store, session_store};
use crate::ui::components::csv_export_dialog::{
    show_csv_export_options_dialog, show_csv_save_dialog,
};
use crate::ui::components::{
    connection_dialog::{ConnectionDialog, ConnectionDialogInit, ConnectionDialogOutput},
    database_selector::{DatabaseSelector, DatabaseSelectorMsg, DatabaseSelectorOutput},
    editor::{SqlEditor, SqlEditorMsg, SqlEditorOutput},
    results::{QueryResults, QueryResultsMsg, QueryResultsOutput},
    sidebar::{ObjectSidebar, ObjectSidebarMsg, ObjectSidebarOutput},
    start_screen::{StartScreen, StartScreenMsg, StartScreenOutput},
    table_view::{TableView, TableViewOutput},
};

mod database_switching;
mod object_actions;
mod query_execution;
mod tabs;

use object_actions::ObjectActionRequest;
use query_execution::{QueryExecutionContext, QueryState, RunningQuery};
use tabs::{browse_tab_id_from_widget, query_tab_id_from_widget, setup_tab_context_menu};

static WINDOW_CONTENT_SUBSCRIBERS: Mutex<Option<HashMap<u64, Sender<WindowContentMsg>>>> =
    Mutex::new(None);
static NEXT_WINDOW_CONTENT_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(0);

pub struct WindowContent {
    state: AppState,
    active_pool: Option<PgPool>,
    active_connection_details: Option<ConnectionDetails>,
    connection_dialog: Option<Controller<ConnectionDialog>>,
    database_selector: Controller<DatabaseSelector>,
    visible_page: VisiblePage,
    start_screen: Controller<StartScreen>,
    sidebar: Controller<ObjectSidebar>,
    query_tabs: Vec<QueryTab>,
    browse_tabs: Vec<BrowseTab>,
    query_history: Vec<QueryHistoryEntry>,
    active_query_tab_id: u64,
    next_query_tab_id: u64,
    next_browse_tab_id: u64,
    menu_button: gtk::MenuButton,
    active_schema_request_id: Option<u64>,
    next_schema_request_id: u64,
    active_database_list_request_id: Option<u64>,
    next_database_list_request_id: u64,
    active_database_switch_request_id: Option<u64>,
    next_database_switch_request_id: u64,
    table_script_generation: u64,
    next_query_id: u64,
    workspace_navigation: WorkspaceNavigation,
    sync_subscription_id: u64,
    session_save_scheduled: bool,
    tab_view_signals_blocked: Rc<Cell<bool>>,
}

struct QueryTab {
    id: u64,
    page: adw::TabPage,
    editor: Controller<SqlEditor>,
    results: Controller<QueryResults>,
    editor_buffer: sourceview5::Buffer,
    row_limit: usize,
    query_state: QueryState,
    active_query: Option<RunningQuery>,
}

struct BrowseTab {
    id: u64,
    page: adw::TabPage,
    object: DatabaseObject,
    stack: gtk::Stack,
    view: Option<Controller<TableView>>,
    loaded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisiblePage {
    Start,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceNavigation {
    SidebarVisible,
    SidebarHidden,
}

#[derive(Debug)]
pub enum WindowContentMsg {
    OpenConnectionDialog,
    ShowStartScreen,
    NewQueryTab,
    SelectQueryTab(u64),
    SelectBrowseTab(u64),
    CloseQueryTab(u64),
    CloseBrowseTab(u64),
    CloseTabFromMenu(Option<String>),
    CloseOtherTabsFromMenu(Option<String>),
    CloseAllTabs,
    QueryTabTitleChanged(u64),
    RunQuery,
    RefreshActiveBrowseTab,
    FocusEditor,
    FocusObjectSearch,
    ToggleSidebar,
    DatabaseSwitchCompleted {
        id: u64,
        database: String,
        result: Result<PgPool, String>,
    },
    DatabaseSelectorOutput(DatabaseSelectorOutput),
    ConnectionDialogOutput(ConnectionDialogOutput),
    StartScreenOutput(StartScreenOutput),
    SidebarOutput(ObjectSidebarOutput),
    ObjectActionConfirmed(ObjectActionRequest),
    ObjectActionCompleted(ObjectActionRequest, Result<(), String>),
    TableScriptGenerated {
        generation: u64,
        kind: TableScriptKind,
        result: Result<String, String>,
    },
    CredentialWarning(String),
    DisableSavedPassword(String),
    EditorOutput {
        tab_id: u64,
        output: SqlEditorOutput,
    },
    ResultsOutput {
        tab_id: u64,
        output: QueryResultsOutput,
    },
    QueryCsvExportConfirmed {
        pool: PgPool,
        sql: String,
        options: CsvExportOptions,
        path: PathBuf,
    },
    BrowseTabOutput {
        tab_id: u64,
        output: TableViewOutput,
    },
    WindowEvent(WindowContentEvent),
    SaveSession,
}

#[derive(Debug, Clone)]
pub enum WindowContentEvent {
    Connections(Vec<SavedConnection>),
    QueryHistory {
        connection_id: String,
        database: String,
    },
    Schema {
        connection_id: String,
        database: String,
    },
}

#[derive(Debug)]
pub enum WindowContentCommandOutput {
    SchemaLoaded {
        id: u64,
        result: Result<Vec<DatabaseObject>, String>,
    },
    DatabasesLoaded {
        id: u64,
        result: Result<Vec<String>, String>,
    },
    DatabaseSwitched {
        id: u64,
        database: String,
        result: Result<PgPool, String>,
    },
    QueryExecuted {
        tab_id: u64,
        id: u64,
        context: QueryExecutionContext,
        result: Result<QueryExecutionResult, String>,
    },
    QueryCancelled {
        tab_id: u64,
        id: u64,
    },
    QueryCsvExported(Result<(), String>),
    ObjectActionFinished(ObjectActionRequest, Result<(), String>),
    TableScriptGenerated {
        generation: u64,
        kind: TableScriptKind,
        result: Result<String, String>,
    },
    SavedPasswordUpdated {
        connection_id: String,
        save_password: bool,
        result: Result<(), String>,
    },
}

#[relm4::component(pub)]
impl Component for WindowContent {
    type Init = ();
    type Input = WindowContentMsg;
    type Output = ();
    type CommandOutput = WindowContentCommandOutput;

    view! {
        root = adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                    pack_start = &gtk::Button {
                        set_icon_name: "go-previous-symbolic",
                        set_tooltip_text: Some(&gettext("Back to connections")),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.shows_workspace(),
                        connect_clicked => WindowContentMsg::ShowStartScreen,
                    },

                    pack_start = &gtk::Button {
                        set_icon_name: "sidebar-show-symbolic",
                        #[watch]
                        set_tooltip_text: Some(&model.sidebar_toggle_tooltip()),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.shows_workspace(),
                        connect_clicked => WindowContentMsg::ToggleSidebar,
                    },

                    pack_end = &model.menu_button.clone() {
                        set_icon_name: "open-menu-symbolic",
                        set_tooltip_text: Some(&gettext("Main Menu")),
                        set_primary: true,
                        set_menu_model: Some(&menus::start_menu()),
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "tab-new-symbolic",
                        set_tooltip_text: Some(&gettext("New Query Tab")),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.shows_workspace(),
                        connect_clicked => WindowContentMsg::NewQueryTab,
                    },

                    #[wrap(Some)]
                    set_title_widget = model.database_selector.widget(),
            },

            #[wrap(Some)]
            #[name = "toast_overlay"]
            set_content = &adw::ToastOverlay {
                #[wrap(Some)]
                #[name = "content_stack"]
                set_child = &gtk::Stack {
                    add_named[Some("start")] = model.start_screen.widget(),

                    add_named[Some("workspace")] = &adw::BreakpointBin {
                        set_width_request: 360,
                        set_height_request: 320,

                        #[wrap(Some)]
                        set_child = &adw::OverlaySplitView {
                            set_sidebar_width_fraction: 0.22,
                            set_min_sidebar_width: 220.0,
                            set_max_sidebar_width: 320.0,
                            set_pin_sidebar: true,
                            set_enable_show_gesture: true,
                            set_enable_hide_gesture: true,
                            #[wrap(Some)]
                            set_sidebar = model.sidebar.widget(),

                            #[wrap(Some)]
                            set_content = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 0,

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Horizontal,
                                    #[watch]
                                    set_visible: model.shows_workspace() && model.has_multiple_tabs(),

                                    adw::TabBar {
                                        set_hexpand: true,
                                        set_autohide: false,
                                        set_view: Some(&query_tab_view),
                                    },
                                },

                                #[name = "query_tab_view"]
                                adw::TabView {
                                    set_vexpand: true,
                                },
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let connections = connection_store::load_connections();
        let start_screen = StartScreen::builder()
            .launch(connections.clone())
            .forward(sender.input_sender(), WindowContentMsg::StartScreenOutput);
        let sidebar = ObjectSidebar::builder()
            .launch(())
            .forward(sender.input_sender(), WindowContentMsg::SidebarOutput);
        let database_selector = DatabaseSelector::builder().launch(()).forward(
            sender.input_sender(),
            WindowContentMsg::DatabaseSelectorOutput,
        );

        let tab_view_signals_blocked = Rc::new(Cell::new(false));
        let mut model = WindowContent {
            state: AppState {
                connections,
                ..AppState::default()
            },
            active_pool: None,
            active_connection_details: None,
            connection_dialog: None,
            database_selector,
            visible_page: VisiblePage::Start,
            start_screen,
            sidebar,
            query_tabs: Vec::new(),
            browse_tabs: Vec::new(),
            query_history: Vec::new(),
            active_query_tab_id: 0,
            next_query_tab_id: 0,
            next_browse_tab_id: 0,
            menu_button: gtk::MenuButton::new(),
            active_schema_request_id: None,
            next_schema_request_id: 0,
            active_database_list_request_id: None,
            next_database_list_request_id: 0,
            active_database_switch_request_id: None,
            next_database_switch_request_id: 0,
            table_script_generation: 0,
            next_query_id: 0,
            workspace_navigation: WorkspaceNavigation::SidebarVisible,
            sync_subscription_id: subscribe_window_content(sender.input_sender().clone()),
            session_save_scheduled: false,
            tab_view_signals_blocked: tab_view_signals_blocked.clone(),
        };
        let widgets = view_output!();

        model.add_query_tab(&widgets, &sender);
        setup_tab_context_menu(&widgets.query_tab_view, &sender);
        widgets.content_stack.set_visible_child_name("start");
        setup_workspace_breakpoint(&widgets);

        let s = sender.clone();
        let selected_page_signals_blocked = tab_view_signals_blocked.clone();
        widgets
            .query_tab_view
            .connect_selected_page_notify(move |tab_view| {
                if selected_page_signals_blocked.get() {
                    return;
                }

                let Some(page) = tab_view.selected_page() else {
                    return;
                };

                if let Some(tab_id) = query_tab_id_from_widget(&page.child()) {
                    s.input(WindowContentMsg::SelectQueryTab(tab_id));
                } else if let Some(tab_id) = browse_tab_id_from_widget(&page.child()) {
                    s.input(WindowContentMsg::SelectBrowseTab(tab_id));
                }
            });

        let s = sender.clone();
        let close_page_signals_blocked = tab_view_signals_blocked.clone();
        widgets.query_tab_view.connect_close_page(move |_, page| {
            if close_page_signals_blocked.get() {
                return glib::Propagation::Stop;
            }

            if let Some(tab_id) = query_tab_id_from_widget(&page.child()) {
                s.input(WindowContentMsg::CloseQueryTab(tab_id));
            } else if let Some(tab_id) = browse_tab_id_from_widget(&page.child()) {
                s.input(WindowContentMsg::CloseBrowseTab(tab_id));
            }
            glib::Propagation::Stop
        });

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match msg {
            WindowContentMsg::ShowStartScreen => self.show_start_screen(widgets),
            WindowContentMsg::NewQueryTab => {
                self.add_query_tab_if_workspace_visible(widgets, &sender);
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::SelectQueryTab(tab_id) => {
                if self.query_tabs.iter().any(|tab| tab.id == tab_id) {
                    self.active_query_tab_id = tab_id;
                    self.sidebar.emit(ObjectSidebarMsg::SetSelectedObject(None));
                    self.schedule_session_save(&sender);
                }
            }
            WindowContentMsg::SelectBrowseTab(tab_id) => {
                if let Some(object) = self
                    .browse_tabs
                    .iter()
                    .find(|tab| tab.id == tab_id)
                    .map(|tab| tab.object.clone())
                {
                    self.sidebar
                        .emit(ObjectSidebarMsg::SetSelectedObject(Some(object)));
                    self.load_browse_tab_if_needed(tab_id, &sender);
                    self.schedule_session_save(&sender);
                }
            }
            WindowContentMsg::CloseQueryTab(tab_id) => {
                self.close_query_tab(tab_id, widgets);
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::CloseBrowseTab(tab_id) => {
                self.close_browse_tab(tab_id, widgets);
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::CloseTabFromMenu(widget_name) => {
                self.close_tab_from_widget_name(widget_name.as_deref(), widgets);
            }
            WindowContentMsg::CloseOtherTabsFromMenu(widget_name) => {
                self.close_other_tabs_from_widget_name(widget_name.as_deref(), widgets);
            }
            WindowContentMsg::CloseAllTabs => {
                self.close_all_tabs(widgets, &sender);
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::QueryTabTitleChanged(tab_id) => {
                self.update_query_tab_title(tab_id);
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::OpenConnectionDialog => {
                if !self.shows_workspace() {
                    self.open_connection_dialog(root, &sender, None);
                }
            }
            WindowContentMsg::StartScreenOutput(StartScreenOutput::NewConnection) => {
                self.open_connection_dialog(root, &sender, None);
            }
            WindowContentMsg::StartScreenOutput(StartScreenOutput::ConnectionsChanged(
                connections,
            )) => {
                self.state.connections.clone_from(&connections);
                self.broadcast_connections_changed(connections);
            }
            WindowContentMsg::StartScreenOutput(StartScreenOutput::OpenConnection(connection)) => {
                self.open_connection_dialog(root, &sender, Some(connection));
            }
            WindowContentMsg::RunQuery => {
                self.run_selected_query_tab(widgets, &sender);
            }
            WindowContentMsg::RefreshActiveBrowseTab => {
                self.refresh_active_browse_tab(widgets, &sender);
            }
            WindowContentMsg::EditorOutput {
                tab_id,
                output: SqlEditorOutput::RunRequested,
            } => {
                self.run_query_for_tab(tab_id, widgets, &sender);
            }
            WindowContentMsg::EditorOutput {
                tab_id,
                output: SqlEditorOutput::CancelRequested,
            } => {
                self.cancel_query(tab_id);
            }
            WindowContentMsg::EditorOutput {
                tab_id,
                output: SqlEditorOutput::HistorySelected(sql),
            } => {
                if let Some(tab) = self.query_tab_mut(tab_id) {
                    tab.editor_buffer.set_text(&sql);
                    tab.editor.emit(SqlEditorMsg::Focus);
                }
            }
            WindowContentMsg::EditorOutput {
                output: SqlEditorOutput::ClearHistoryRequested,
                ..
            } => {
                self.clear_query_history(widgets);
            }
            WindowContentMsg::ResultsOutput {
                tab_id,
                output: QueryResultsOutput::RowLimitChanged(row_limit),
            } => {
                if let Some(tab) = self.query_tab_mut(tab_id) {
                    tab.row_limit = row_limit;
                }
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::ResultsOutput {
                output: QueryResultsOutput::Copied(message),
                ..
            } => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&message));
            }
            WindowContentMsg::ResultsOutput {
                tab_id,
                output: QueryResultsOutput::ExportCsvRequested,
            } => {
                self.open_query_export_dialog(tab_id, root, widgets, &sender);
            }
            WindowContentMsg::QueryCsvExportConfirmed {
                pool,
                sql,
                options,
                path,
            } => {
                self.export_query_csv(pool, sql, options, path, &sender);
            }
            WindowContentMsg::BrowseTabOutput {
                tab_id,
                output: TableViewOutput::Copied(message),
            } => {
                if self.browse_tabs.iter().any(|tab| tab.id == tab_id) {
                    widgets.toast_overlay.add_toast(adw::Toast::new(&message));
                }
            }
            WindowContentMsg::BrowseTabOutput {
                tab_id,
                output: TableViewOutput::Exported(message),
            } => {
                if self.browse_tabs.iter().any(|tab| tab.id == tab_id) {
                    widgets.toast_overlay.add_toast(adw::Toast::new(&message));
                }
            }
            WindowContentMsg::WindowEvent(event) => {
                self.handle_window_event(event, widgets, &sender);
            }
            WindowContentMsg::SaveSession => {
                self.session_save_scheduled = false;
                if let Err(error) = self.save_current_session(widgets) {
                    widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                        "{}: {error}",
                        gettext("Saving tabs failed")
                    )));
                }
            }
            WindowContentMsg::FocusEditor => {
                self.select_active_query_tab(widgets, &sender);

                if let Some(tab) = self.active_query_tab_mut() {
                    tab.editor.emit(SqlEditorMsg::Focus);
                }
            }
            WindowContentMsg::FocusObjectSearch => {
                self.focus_object_search(widgets);
            }
            WindowContentMsg::ToggleSidebar => self.toggle_sidebar(widgets, root),
            WindowContentMsg::DatabaseSelectorOutput(DatabaseSelectorOutput::DatabaseSelected(
                database,
            )) => {
                self.switch_database(database, widgets, &sender);
            }
            WindowContentMsg::DatabaseSwitchCompleted {
                id,
                database,
                result,
            } => {
                self.handle_database_switched(id, database, result, widgets, &sender);
            }
            WindowContentMsg::ConnectionDialogOutput(ConnectionDialogOutput::Connected {
                details,
                pool,
            }) => self.handle_connected(details, pool, widgets, &sender, root),
            WindowContentMsg::ConnectionDialogOutput(ConnectionDialogOutput::Dismissed) => {
                self.connection_dialog = None;
            }
            WindowContentMsg::SidebarOutput(ObjectSidebarOutput::OpenObject(object)) => {
                self.open_table_browser(object, widgets, &sender);
                self.schedule_session_save(&sender);
            }
            WindowContentMsg::SidebarOutput(ObjectSidebarOutput::CopyText { text, message }) => {
                copy_text_to_clipboard(&text);
                widgets.toast_overlay.add_toast(adw::Toast::new(&message));
            }
            WindowContentMsg::SidebarOutput(ObjectSidebarOutput::ObjectAction {
                object,
                action,
            }) => {
                self.handle_object_action(object, action, widgets, &sender);
            }
            WindowContentMsg::SidebarOutput(ObjectSidebarOutput::TableScriptRequested {
                object,
                kind,
            }) => {
                self.generate_table_script(object, kind, widgets, &sender);
            }
            WindowContentMsg::ObjectActionConfirmed(request) => {
                self.run_object_action(request, widgets, &sender);
            }
            WindowContentMsg::ObjectActionCompleted(request, result) => {
                self.handle_object_action_completed(request, result, widgets, &sender);
            }
            WindowContentMsg::TableScriptGenerated {
                generation,
                kind,
                result,
            } => {
                self.handle_table_script_generated(generation, kind, result, widgets, &sender);
            }
            WindowContentMsg::CredentialWarning(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Updating the saved password failed")
                )));
            }
            WindowContentMsg::DisableSavedPassword(connection_id) => {
                self.disable_saved_password(&connection_id, widgets);
            }
        }

        self.update_view(widgets, sender);
    }

    fn update_cmd_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            WindowContentCommandOutput::SchemaLoaded { id, result } => {
                self.handle_schema_loaded(id, result);
            }
            WindowContentCommandOutput::DatabasesLoaded { id, result } => {
                self.handle_databases_loaded(id, result);
            }
            WindowContentCommandOutput::DatabaseSwitched {
                id,
                database,
                result,
            } => {
                sender.input(WindowContentMsg::DatabaseSwitchCompleted {
                    id,
                    database,
                    result,
                });
            }
            WindowContentCommandOutput::QueryExecuted {
                tab_id,
                id,
                context,
                result,
            } => {
                self.handle_query_executed(tab_id, id, context, result, widgets, &sender);
            }
            WindowContentCommandOutput::QueryCancelled { tab_id, id } => {
                self.handle_query_cancelled(tab_id, id);
            }
            WindowContentCommandOutput::QueryCsvExported(result) => match result {
                Ok(()) => {
                    widgets
                        .toast_overlay
                        .add_toast(adw::Toast::new(&gettext("CSV exported.")));
                }
                Err(error) => {
                    show_export_error_dialog(widgets, &error);
                }
            },
            WindowContentCommandOutput::ObjectActionFinished(request, result) => {
                sender.input(WindowContentMsg::ObjectActionCompleted(request, result));
            }
            WindowContentCommandOutput::TableScriptGenerated {
                generation,
                kind,
                result,
            } => {
                sender.input(WindowContentMsg::TableScriptGenerated {
                    generation,
                    kind,
                    result,
                });
            }
            WindowContentCommandOutput::SavedPasswordUpdated {
                connection_id,
                save_password,
                result,
            } => {
                if let Err(error) = result {
                    if save_password {
                        sender.input(WindowContentMsg::DisableSavedPassword(connection_id));
                    }

                    sender.input(WindowContentMsg::CredentialWarning(error));
                }
            }
        }

        self.update_view(widgets, sender);
    }

    fn shutdown(&mut self, widgets: &mut Self::Widgets, _output: Sender<Self::Output>) {
        let _ = self.save_current_session(widgets);
        unsubscribe_window_content(self.sync_subscription_id);
    }
}

fn with_window_content_subscribers<R>(
    f: impl FnOnce(&mut HashMap<u64, Sender<WindowContentMsg>>) -> R,
) -> R {
    let mut guard = WINDOW_CONTENT_SUBSCRIBERS.lock().expect("subscribers lock");
    let subscribers = guard.get_or_insert_with(HashMap::new);
    f(subscribers)
}

fn subscribe_window_content(sender: Sender<WindowContentMsg>) -> u64 {
    let id = NEXT_WINDOW_CONTENT_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed);

    with_window_content_subscribers(|subscribers| {
        subscribers.insert(id, sender);
    });

    id
}

fn unsubscribe_window_content(id: u64) {
    with_window_content_subscribers(|subscribers| {
        subscribers.remove(&id);
    });
}

fn broadcast_window_content_event(event: WindowContentEvent, except: Option<u64>) {
    with_window_content_subscribers(|subscribers| {
        subscribers.retain(|id, sender| {
            if Some(*id) == except {
                return true;
            }

            sender
                .send(WindowContentMsg::WindowEvent(event.clone()))
                .is_ok()
        });
    });
}

pub(super) fn workspace_split_view(widgets: &WindowContentWidgets) -> adw::OverlaySplitView {
    let breakpoint_bin = widgets
        .content_stack
        .child_by_name("workspace")
        .and_downcast::<adw::BreakpointBin>()
        .expect("workspace breakpoint bin to exist");

    breakpoint_bin
        .child()
        .and_downcast::<adw::OverlaySplitView>()
        .expect("workspace split view to exist")
}

fn setup_workspace_breakpoint(widgets: &WindowContentWidgets) {
    let split_view = workspace_split_view(widgets);
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        700.0,
        adw::LengthUnit::Sp,
    ));
    breakpoint.add_setters(&[
        (&split_view, "collapsed", true),
        (&split_view, "pin-sidebar", false),
    ]);

    widgets
        .content_stack
        .child_by_name("workspace")
        .and_downcast::<adw::BreakpointBin>()
        .expect("workspace breakpoint bin to exist")
        .add_breakpoint(breakpoint);
}

fn copy_text_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

async fn update_saved_password(details: ConnectionDetails) -> Result<(), String> {
    if details.saved.save_password {
        if details.password.is_empty() {
            return Ok(());
        }

        credential_store::store_password(&details.saved, &details.password)
            .await
            .map_err(|error| error.to_string())?;
    } else {
        credential_store::delete_password(&details.saved.id)
            .await
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

impl WorkspaceNavigation {
    fn from_sidebar_visibility(show_sidebar: bool) -> Self {
        if show_sidebar {
            Self::SidebarVisible
        } else {
            Self::SidebarHidden
        }
    }

    fn sidebar_toggle_tooltip(self) -> String {
        match self {
            Self::SidebarVisible => gettext("Hide Objects"),
            Self::SidebarHidden => gettext("Show Objects"),
        }
    }
}

impl WindowContent {
    fn handle_window_event(
        &mut self,
        event: WindowContentEvent,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        match event {
            WindowContentEvent::Connections(connections) => {
                self.apply_external_connections(connections, widgets);
            }
            WindowContentEvent::QueryHistory {
                connection_id,
                database,
            } => {
                self.reload_query_history_if_active(&connection_id, &database);
            }
            WindowContentEvent::Schema {
                connection_id,
                database,
            } => {
                if self.matches_active_database(&connection_id, &database) {
                    self.reload_schema(sender);
                }
            }
        }
    }

    fn apply_external_connections(
        &mut self,
        connections: Vec<SavedConnection>,
        widgets: &mut WindowContentWidgets,
    ) {
        let active_connection_id = self
            .state
            .active_connection
            .as_ref()
            .map(|connection| connection.id.clone());

        if let Some(active_connection_id) = active_connection_id {
            let updated = connections
                .iter()
                .find(|connection| connection.id == active_connection_id);

            if let Some(updated) = updated {
                if let Some(active_connection) = self.state.active_connection.as_mut() {
                    active_connection.name.clone_from(&updated.name);
                    active_connection.save_password = updated.save_password;
                }

                if let Some(details) = self.active_connection_details.as_mut() {
                    details.saved.name.clone_from(&updated.name);
                    details.saved.save_password = updated.save_password;
                }

                let active_database = self
                    .state
                    .active_database
                    .clone()
                    .unwrap_or_else(|| updated.database.clone());
                self.database_selector
                    .emit(DatabaseSelectorMsg::SetContext {
                        connection_title: updated.name.clone(),
                        active_database,
                        databases: self
                            .databases_with_active_database(self.state.available_databases.clone()),
                    });
            } else if self.shows_workspace() {
                self.show_start_screen(widgets);
            }
        }

        self.state.connections.clone_from(&connections);
        self.start_screen
            .emit(StartScreenMsg::SetConnections(connections));
    }

    pub(super) fn broadcast_connections_changed(&self, connections: Vec<SavedConnection>) {
        broadcast_window_content_event(
            WindowContentEvent::Connections(connections),
            Some(self.sync_subscription_id),
        );
    }

    pub(super) fn broadcast_query_history_changed(&self) {
        let Some((connection_id, database)) = self.active_database_scope() else {
            return;
        };

        broadcast_window_content_event(
            WindowContentEvent::QueryHistory {
                connection_id,
                database,
            },
            Some(self.sync_subscription_id),
        );
    }

    pub(super) fn broadcast_schema_changed(&self) {
        let Some((connection_id, database)) = self.active_database_scope() else {
            return;
        };

        broadcast_window_content_event(
            WindowContentEvent::Schema {
                connection_id,
                database,
            },
            Some(self.sync_subscription_id),
        );
    }

    fn active_database_scope(&self) -> Option<(String, String)> {
        let connection_id = self.state.active_connection.as_ref()?.id.clone();
        let database = self.state.active_database.clone()?;

        Some((connection_id, database))
    }

    fn matches_active_database(&self, connection_id: &str, database: &str) -> bool {
        self.state
            .active_connection
            .as_ref()
            .is_some_and(|connection| connection.id == connection_id)
            && self.state.active_database.as_deref() == Some(database)
    }

    fn shows_workspace(&self) -> bool {
        self.visible_page == VisiblePage::Workspace
    }

    fn sidebar_toggle_tooltip(&self) -> String {
        self.workspace_navigation.sidebar_toggle_tooltip()
    }

    fn persist_sidebar_hidden(&self, hidden: bool) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        let settings = settings::connection_state_settings(&connection.id);
        let _ = settings.set_boolean("sidebar-hidden", hidden);
    }

    fn show_start_screen(&mut self, widgets: &mut WindowContentWidgets) {
        if let Err(error) = self.save_current_session(widgets) {
            widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                "{}: {error}",
                gettext("Saving tabs failed")
            )));
        }

        self.visible_page = VisiblePage::Start;
        self.database_selector
            .emit(DatabaseSelectorMsg::SetContext {
                connection_title: String::new(),
                active_database: String::new(),
                databases: Vec::new(),
            });
        self.database_selector
            .emit(DatabaseSelectorMsg::SetLoading(false));

        self.active_pool = None;
        self.active_connection_details = None;
        self.cancel_all_queries();
        self.active_schema_request_id = None;
        self.active_database_list_request_id = None;
        self.active_database_switch_request_id = None;
        self.advance_table_script_generation();
        self.state.active_connection = None;
        self.state.active_database = None;
        self.state.available_databases.clear();
        self.state.objects.clear();
        self.query_history.clear();
        self.clear_browse_tabs(widgets);

        for tab in &self.query_tabs {
            tab.editor_buffer.set_text("");
            tab.editor.emit(SqlEditorMsg::SetHistory(Vec::new()));
            tab.results.emit(QueryResultsMsg::Clear);
        }

        self.sidebar
            .emit(ObjectSidebarMsg::SetError(gettext("No connection")));
        self.menu_button.set_menu_model(Some(&menus::start_menu()));
        widgets.content_stack.set_visible_child_name("start");
    }

    fn toggle_sidebar(&mut self, widgets: &WindowContentWidgets, _root: &adw::ToolbarView) {
        let split_view = workspace_split_view(widgets);
        let show_sidebar = !split_view.shows_sidebar();
        split_view.set_show_sidebar(show_sidebar);
        let hide_sidebar = !show_sidebar;
        self.persist_sidebar_hidden(hide_sidebar);
        self.workspace_navigation = WorkspaceNavigation::from_sidebar_visibility(show_sidebar);
    }

    fn focus_object_search(&mut self, widgets: &WindowContentWidgets) {
        if !self.shows_workspace() {
            return;
        }

        let split_view = workspace_split_view(widgets);
        split_view.set_show_sidebar(true);
        self.workspace_navigation = WorkspaceNavigation::from_sidebar_visibility(true);
        self.sidebar.emit(ObjectSidebarMsg::FocusSearch);
    }

    fn handle_connected(
        &mut self,
        details: ConnectionDetails,
        pool: PgPool,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
        root: &adw::ToolbarView,
    ) {
        self.cancel_all_queries();
        self.advance_table_script_generation();

        let connection = details.saved.clone();
        self.state.active_connection = Some(connection.clone());
        self.state.active_database = Some(connection.database.clone());
        self.state.available_databases.clear();
        self.database_selector
            .emit(DatabaseSelectorMsg::SetContext {
                connection_title: connection.name.clone(),
                active_database: connection.database.clone(),
                databases: vec![connection.database.clone()],
            });
        self.database_selector
            .emit(DatabaseSelectorMsg::SetLoading(true));
        self.active_pool = Some(pool.clone());
        self.active_connection_details = Some(details.clone());
        self.connection_dialog = None;
        self.visible_page = VisiblePage::Workspace;
        self.migrate_legacy_query_history(&connection, widgets);
        self.load_query_history(&connection);
        self.sidebar.emit(ObjectSidebarMsg::Loading);

        self.show_workspace(widgets, root, &connection);
        self.restore_saved_session_or_default(widgets, sender);

        match connection_store::save_connection(&connection) {
            Ok(connections) => {
                self.state.connections.clone_from(&connections);
                self.start_screen
                    .emit(StartScreenMsg::SetConnections(connections.clone()));
                self.broadcast_connections_changed(connections);
                widgets
                    .toast_overlay
                    .add_toast(adw::Toast::new(&gettext("Connected to PostgreSQL.")));

                sender.oneshot_command(async move {
                    WindowContentCommandOutput::SavedPasswordUpdated {
                        connection_id: details.saved.id.clone(),
                        save_password: details.saved.save_password,
                        result: update_saved_password(details).await,
                    }
                });
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Connected, but saving the connection failed")
                )));
            }
        }

        let schema_request_id = self.allocate_schema_request_id();
        self.active_schema_request_id = Some(schema_request_id);

        let database_list_request_id = self.allocate_database_list_request_id();
        self.active_database_list_request_id = Some(database_list_request_id);

        let database_pool = pool.clone();
        sender.oneshot_command(async move {
            WindowContentCommandOutput::DatabasesLoaded {
                id: database_list_request_id,
                result: db::postgres::list_databases(&database_pool)
                    .await
                    .map_err(|error| error.to_string()),
            }
        });

        sender.oneshot_command(async move {
            WindowContentCommandOutput::SchemaLoaded {
                id: schema_request_id,
                result: db::schema::load_schema(&pool)
                    .await
                    .map_err(|error| error.to_string()),
            }
        });
    }

    fn show_workspace(
        &mut self,
        widgets: &mut WindowContentWidgets,
        _root: &adw::ToolbarView,
        connection: &SavedConnection,
    ) {
        let split_view = workspace_split_view(widgets);
        let sidebar_hidden =
            settings::connection_state_settings(&connection.id).boolean("sidebar-hidden");
        split_view.set_show_sidebar(!sidebar_hidden);
        self.workspace_navigation =
            WorkspaceNavigation::from_sidebar_visibility(split_view.shows_sidebar());
        self.menu_button
            .set_menu_model(Some(&menus::workspace_menu()));
        widgets.content_stack.set_visible_child_name("workspace");
    }

    fn schedule_session_save(&mut self, sender: &ComponentSender<Self>) {
        if !self.shows_workspace() || self.session_save_scheduled {
            return;
        }

        self.session_save_scheduled = true;
        let sender = sender.input_sender().clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let _ = sender.send(WindowContentMsg::SaveSession);
        });
    }

    fn save_current_session(&self, widgets: &WindowContentWidgets) -> Result<(), String> {
        let Some(session) = self.saved_session(widgets) else {
            return Ok(());
        };

        session_store::save(&session).map_err(|error| error.to_string())
    }

    fn saved_session(&self, widgets: &WindowContentWidgets) -> Option<SavedSession> {
        let connection_id = self.state.active_connection.as_ref()?.id.clone();
        let database = self.state.active_database.clone()?;

        Some(self.build_saved_session(connection_id, database, widgets))
    }

    fn restore_saved_session_or_default(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let session = self
            .state
            .active_connection
            .as_ref()
            .zip(self.state.active_database.as_ref())
            .and_then(|(connection, database)| session_store::load(&connection.id, database));

        self.restore_session(session, widgets, sender);
    }

    fn disable_saved_password(&mut self, connection_id: &str, widgets: &WindowContentWidgets) {
        match connection_store::set_save_password(connection_id, false) {
            Ok(connections) => {
                self.state.connections.clone_from(&connections);
                self.start_screen
                    .emit(StartScreenMsg::SetConnections(connections.clone()));
                self.broadcast_connections_changed(connections);

                if let Some(connection) = self
                    .state
                    .active_connection
                    .as_mut()
                    .filter(|connection| connection.id == connection_id)
                {
                    connection.save_password = false;
                }

                if let Some(details) = self
                    .active_connection_details
                    .as_mut()
                    .filter(|details| details.saved.id == connection_id)
                {
                    details.saved.save_password = false;
                }
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Disabling saved password failed")
                )));
            }
        }
    }

    fn open_query_export_dialog(
        &self,
        tab_id: u64,
        root: &adw::ToolbarView,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pool) = self.active_pool.clone() else {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Connect to PostgreSQL before exporting query results.",
            )));
            return;
        };

        let sql = self.query_tab_execution_sql(tab_id).unwrap_or_default();
        if sql.trim().is_empty() {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Enter SQL before exporting query results.",
            )));
            return;
        }

        let parent = root.root().and_downcast::<gtk::Window>();
        let sender = sender.clone();

        show_csv_export_options_dialog(parent.clone().as_ref(), move |options| {
            let parent = parent.clone();
            let sender = sender.clone();
            let pool = pool.clone();
            let sql = sql.clone();

            show_csv_save_dialog(
                parent.as_ref(),
                "query-results.csv".to_string(),
                move |path| {
                    sender.input(WindowContentMsg::QueryCsvExportConfirmed {
                        pool,
                        sql,
                        options,
                        path,
                    });
                },
            );
        });
    }

    fn export_query_csv(
        &self,
        pool: PgPool,
        sql: String,
        options: CsvExportOptions,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        sender.oneshot_command(async move {
            let result = async {
                let result = db::query::execute_read_only(&pool, &sql, options.row_limit)
                    .await
                    .map_err(|error| query_export_error_message(&error.to_string()))?;

                let QueryExecutionResult::Rows(result) = result else {
                    return Err(gettext("The query did not return rows."));
                };

                let csv = csv_export::query_result(&result, options);

                std::fs::write(path, csv).map_err(|error| error.to_string())
            }
            .await;

            WindowContentCommandOutput::QueryCsvExported(result)
        });
    }

    fn handle_schema_loaded(&mut self, id: u64, result: Result<Vec<DatabaseObject>, String>) {
        if self.active_schema_request_id != Some(id) {
            return;
        }

        self.active_schema_request_id = None;

        match result {
            Ok(objects) => {
                self.state.objects.clone_from(&objects);
                self.sidebar.emit(ObjectSidebarMsg::SetObjects(objects));
            }
            Err(error) => {
                self.sidebar.emit(ObjectSidebarMsg::SetError(format!(
                    "{}: {error}",
                    gettext("Schema load failed")
                )));
            }
        }
    }

    fn reload_schema(&mut self, sender: &ComponentSender<Self>) {
        let Some(pool) = self.active_pool.clone() else {
            return;
        };

        let schema_request_id = self.allocate_schema_request_id();
        self.active_schema_request_id = Some(schema_request_id);

        sender.oneshot_command(async move {
            WindowContentCommandOutput::SchemaLoaded {
                id: schema_request_id,
                result: db::schema::load_schema(&pool)
                    .await
                    .map_err(|error| error.to_string()),
            }
        });
    }

    fn generate_table_script(
        &mut self,
        object: DatabaseObject,
        kind: TableScriptKind,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pool) = self.active_pool.clone() else {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Connect to PostgreSQL before generating a script.",
            )));
            return;
        };

        let generation = self.table_script_generation;

        sender.oneshot_command(async move {
            WindowContentCommandOutput::TableScriptGenerated {
                generation,
                kind,
                result: db::table_scripts::generate_table_script(&pool, &object, kind)
                    .await
                    .map_err(|error| error.to_string()),
            }
        });
    }

    fn handle_table_script_generated(
        &mut self,
        generation: u64,
        kind: TableScriptKind,
        result: Result<String, String>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.table_script_generation != generation {
            return;
        }

        match result {
            Ok(sql) => {
                self.add_query_tab_with_sql(sql, widgets, sender);
                widgets
                    .toast_overlay
                    .add_toast(adw::Toast::new(&table_script_generated_message(kind)));
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Generating the table script failed")
                )));
            }
        }
    }

    fn open_connection_dialog(
        &mut self,
        root: &adw::ToolbarView,
        sender: &ComponentSender<Self>,
        connection: Option<SavedConnection>,
    ) {
        if let Some(dialog) = &self.connection_dialog {
            dialog.widget().present();
            return;
        }

        let parent_window = root
            .root()
            .and_downcast::<gtk::Window>()
            .expect("window content to be mounted in a gtk::Window");

        self.connection_dialog = Some(
            ConnectionDialog::builder()
                .launch(ConnectionDialogInit {
                    parent_window,
                    connection,
                })
                .forward(
                    sender.input_sender(),
                    WindowContentMsg::ConnectionDialogOutput,
                ),
        );
    }

    fn allocate_schema_request_id(&mut self) -> u64 {
        let id = self.next_schema_request_id;
        self.next_schema_request_id = self.next_schema_request_id.wrapping_add(1);
        id
    }

    fn advance_table_script_generation(&mut self) {
        self.table_script_generation = self.table_script_generation.wrapping_add(1);
    }
}

fn show_export_error_dialog(widgets: &WindowContentWidgets, error: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Export failed"))
        .body(error)
        .close_response("close")
        .build();

    dialog.add_response("close", &gettext("Close"));
    dialog.present(widgets.toast_overlay.root().as_ref());
}

fn query_export_error_message(error: &str) -> String {
    let lower = error.to_ascii_lowercase();

    if lower.contains("read-only transaction")
        || lower.contains("cannot execute")
        || lower.contains("transaction control statements cannot be exported")
    {
        return format!(
            "{}\n\n{}",
            gettext("CSV export runs queries in a read-only transaction."),
            gettext(
                "Use a SELECT query for export. Statements that write or change transaction state, such as UPDATE, INSERT, DELETE, CREATE, COMMIT, or ROLLBACK, cannot be exported."
            )
        );
    }

    error.to_string()
}

fn table_script_generated_message(kind: TableScriptKind) -> String {
    match kind {
        TableScriptKind::Create => gettext("CREATE script generated."),
        TableScriptKind::Select => gettext("SELECT script generated."),
        TableScriptKind::Insert => gettext("INSERT script generated."),
        TableScriptKind::Update => gettext("UPDATE script generated."),
        TableScriptKind::Delete => gettext("DELETE script generated."),
    }
}
