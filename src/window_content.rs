use futures_util::future::{AbortHandle, Abortable};
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;

use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::menus;
use crate::models::connection::{ConnectionDetails, SavedConnection};
use crate::models::database_object::DatabaseObject;
use crate::models::query_history::QueryHistoryEntry;
use crate::models::query_result::QueryExecutionResult;
use crate::models::table_script::TableScriptKind;
use crate::settings;
use crate::state::{app_state::AppState, connection_store, credential_store};
use crate::ui::components::{
    connection_dialog::{ConnectionDialog, ConnectionDialogInit, ConnectionDialogOutput},
    database_selector::{DatabaseSelector, DatabaseSelectorMsg, DatabaseSelectorOutput},
    editor::{SqlEditor, SqlEditorMsg, SqlEditorOutput},
    results::{QueryResults, QueryResultsMsg},
    sidebar::{ObjectSidebar, ObjectSidebarMsg, ObjectSidebarOutput},
    start_screen::{StartScreen, StartScreenMsg, StartScreenOutput},
    table_view::TableView,
};

mod database_switching;
mod object_actions;
mod tabs;

use object_actions::ObjectActionRequest;
use tabs::{browse_tab_id_from_widget, query_tab_id_from_widget, setup_tab_context_menu};

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
}

struct QueryTab {
    id: u64,
    page: adw::TabPage,
    editor: Controller<SqlEditor>,
    results: Controller<QueryResults>,
    editor_buffer: sourceview5::Buffer,
    query_state: QueryState,
    active_query: Option<RunningQuery>,
}

struct BrowseTab {
    id: u64,
    page: adw::TabPage,
    object: DatabaseObject,
    view: Controller<TableView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisiblePage {
    Start,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryState {
    Idle,
    Running,
}

#[derive(Debug)]
struct RunningQuery {
    id: u64,
    abort_handle: AbortHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceNavigation {
    Wide,
    Content,
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
        reload_schema_on_success: bool,
        result: Result<QueryExecutionResult, String>,
    },
    QueryCancelled {
        tab_id: u64,
        id: u64,
    },
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
                        set_icon_name: "go-previous",
                        set_tooltip_text: Some(&gettext("Back to connections")),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.shows_workspace(),
                        connect_clicked => WindowContentMsg::ShowStartScreen,
                    },

                    pack_start = &gtk::Button {
                        set_icon_name: "sidebar-left",
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
                        set_menu_model: Some(&menus::main_menu()),
                    },

                    pack_end = &gtk::Button {
                        set_tooltip_text: Some(&gettext("New Query Tab")),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.shows_workspace(),
                        set_child: Some(&icon_label_widget("document-edit-regular-symbolic", &gettext("New"))),
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

                    add_named[Some("workspace")] = &adw::NavigationSplitView {
                        set_sidebar_width_fraction: 0.22,
                        set_min_sidebar_width: 220.0,
                        set_max_sidebar_width: 320.0,
                        #[wrap(Some)]
                        set_sidebar = &adw::NavigationPage::builder()
                            .title(gettext("Objects"))
                            .child(model.sidebar.widget())
                            .build(),

                        #[wrap(Some)]
                        set_content = &adw::NavigationPage {
                            set_title: &gettext("Workspace"),

                            #[wrap(Some)]
                            set_child = &gtk::Box {
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
            workspace_navigation: WorkspaceNavigation::Wide,
        };
        let widgets = view_output!();

        model.add_query_tab(&widgets, &sender);
        setup_tab_context_menu(&widgets.query_tab_view, &sender);
        widgets.content_stack.set_visible_child_name("start");
        let workspace_split_view = workspace_split_view(&widgets);
        workspace_split_view.set_show_content(true);

        let s = sender.clone();
        widgets
            .query_tab_view
            .connect_selected_page_notify(move |tab_view| {
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
        widgets.query_tab_view.connect_close_page(move |_, page| {
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
            }
            WindowContentMsg::SelectQueryTab(tab_id) => {
                if self.query_tabs.iter().any(|tab| tab.id == tab_id) {
                    self.active_query_tab_id = tab_id;
                    self.sidebar.emit(ObjectSidebarMsg::SetSelectedObject(None));
                }
            }
            WindowContentMsg::SelectBrowseTab(tab_id) => {
                if let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id) {
                    self.sidebar.emit(ObjectSidebarMsg::SetSelectedObject(Some(
                        tab.object.clone(),
                    )));
                }
            }
            WindowContentMsg::CloseQueryTab(tab_id) => {
                self.close_query_tab(tab_id, widgets);
            }
            WindowContentMsg::CloseBrowseTab(tab_id) => {
                self.close_browse_tab(tab_id, widgets);
            }
            WindowContentMsg::CloseTabFromMenu(widget_name) => {
                self.close_tab_from_widget_name(widget_name.as_deref(), widgets);
            }
            WindowContentMsg::CloseOtherTabsFromMenu(widget_name) => {
                self.close_other_tabs_from_widget_name(widget_name.as_deref(), widgets);
            }
            WindowContentMsg::CloseAllTabs => {
                self.close_all_tabs(widgets, &sender);
            }
            WindowContentMsg::QueryTabTitleChanged(tab_id) => {
                self.update_query_tab_title(tab_id);
            }
            WindowContentMsg::OpenConnectionDialog
            | WindowContentMsg::StartScreenOutput(StartScreenOutput::NewConnection) => {
                self.open_connection_dialog(root, &sender, None);
            }
            WindowContentMsg::StartScreenOutput(StartScreenOutput::ConnectionsChanged(
                connections,
            )) => {
                self.state.connections = connections;
            }
            WindowContentMsg::StartScreenOutput(StartScreenOutput::OpenConnection(connection)) => {
                self.open_connection_dialog(root, &sender, Some(connection));
            }
            WindowContentMsg::RunQuery => {
                self.run_selected_query_tab(widgets, &sender);
            }
            WindowContentMsg::RefreshActiveBrowseTab => {
                self.refresh_active_browse_tab(widgets);
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
                self.open_table_browser(object, widgets);
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

    fn update_cmd(
        &mut self,
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
                reload_schema_on_success,
                result,
            } => {
                self.handle_query_executed(tab_id, id, reload_schema_on_success, result, &sender);
            }
            WindowContentCommandOutput::QueryCancelled { tab_id, id } => {
                self.handle_query_cancelled(tab_id, id);
            }
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
    }
}

fn workspace_split_view(widgets: &WindowContentWidgets) -> adw::NavigationSplitView {
    widgets
        .content_stack
        .child_by_name("workspace")
        .and_downcast::<adw::NavigationSplitView>()
        .expect("workspace split view to exist")
}

fn icon_label_widget(icon_name: &str, label: &str) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let image = gtk::Image::from_icon_name(icon_name);
    container.append(&image);

    let label = gtk::Label::new(Some(label));
    container.append(&label);
    container
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
    fn from_split_view(is_collapsed: bool) -> Self {
        if !is_collapsed {
            Self::Wide
        } else {
            Self::Content
        }
    }

    fn sidebar_toggle_tooltip(self) -> String {
        match self {
            Self::Wide => gettext("Hide Objects"),
            Self::Content => gettext("Show Objects"),
        }
    }
}

impl WindowContent {
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
        widgets.content_stack.set_visible_child_name("start");
    }

    fn toggle_sidebar(&mut self, widgets: &WindowContentWidgets, _root: &adw::ToolbarView) {
        let split_view = workspace_split_view(widgets);
        let hide_sidebar = !split_view.is_collapsed();
        split_view.set_collapsed(hide_sidebar);
        split_view.set_show_content(true);
        self.persist_sidebar_hidden(hide_sidebar);
        self.workspace_navigation = WorkspaceNavigation::from_split_view(hide_sidebar);
    }

    fn focus_object_search(&mut self, widgets: &WindowContentWidgets) {
        if !self.shows_workspace() {
            return;
        }

        let split_view = workspace_split_view(widgets);
        split_view.set_collapsed(false);
        split_view.set_show_content(true);
        self.workspace_navigation = WorkspaceNavigation::from_split_view(false);
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
        self.clear_browse_tabs(widgets);

        for tab in &self.query_tabs {
            tab.editor_buffer.set_text("");
            tab.results.emit(QueryResultsMsg::Clear);
        }
        self.migrate_legacy_query_history(&connection, widgets);
        self.load_query_history(&connection);
        self.sidebar.emit(ObjectSidebarMsg::Loading);

        self.show_workspace(widgets, root, &connection);

        match connection_store::save_connection(&connection) {
            Ok(connections) => {
                self.state.connections.clone_from(&connections);
                self.start_screen
                    .emit(StartScreenMsg::SetConnections(connections));
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
        split_view.set_show_content(true);
        let sidebar_hidden =
            settings::connection_state_settings(&connection.id).boolean("sidebar-hidden");
        split_view.set_collapsed(sidebar_hidden);
        self.workspace_navigation = WorkspaceNavigation::from_split_view(split_view.is_collapsed());
        widgets.content_stack.set_visible_child_name("workspace");
    }

    fn disable_saved_password(&mut self, connection_id: &str, widgets: &WindowContentWidgets) {
        match connection_store::set_save_password(connection_id, false) {
            Ok(connections) => {
                self.state.connections.clone_from(&connections);
                self.start_screen
                    .emit(StartScreenMsg::SetConnections(connections));

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

    fn handle_query_executed(
        &mut self,
        tab_id: u64,
        id: u64,
        reload_schema_on_success: bool,
        result: Result<QueryExecutionResult, String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self.is_active_query(tab_id, id) {
            return;
        }

        let should_reload_schema = {
            let Some(tab) = self.query_tab_mut(tab_id) else {
                return;
            };

            tab.active_query = None;
            tab.query_state = QueryState::Idle;
            tab.editor.emit(SqlEditorMsg::SetRunning(false));

            match result {
                Ok(result) => {
                    tab.results.emit(QueryResultsMsg::ShowResult(result));
                    reload_schema_on_success
                }
                Err(error) => {
                    tab.results.emit(QueryResultsMsg::ShowError(format!(
                        "{}: {error}",
                        gettext("Query failed")
                    )));
                    false
                }
            }
        };

        if should_reload_schema {
            self.reload_schema(sender);
        }
    }

    fn handle_query_cancelled(&mut self, tab_id: u64, id: u64) {
        if !self.is_active_query(tab_id, id) {
            return;
        }

        if let Some(tab) = self.query_tab_mut(tab_id) {
            tab.active_query = None;
            tab.query_state = QueryState::Idle;
            tab.editor.emit(SqlEditorMsg::SetRunning(false));
            tab.results.emit(QueryResultsMsg::Cancelled);
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

    fn run_query_for_tab(
        &mut self,
        tab_id: u64,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.query_tabs.iter().all(|tab| tab.id != tab_id) {
            return;
        }

        if self
            .query_tab_mut(tab_id)
            .is_some_and(|tab| tab.query_state == QueryState::Running)
        {
            widgets
                .toast_overlay
                .add_toast(adw::Toast::new(&gettext("A query is already running.")));
            return;
        }

        let Some(pool) = self.active_pool.clone() else {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Connect to PostgreSQL before running a query.",
            )));
            return;
        };

        let sql = self.query_tab_execution_sql(tab_id).unwrap_or_default();
        if sql.trim().is_empty() {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Enter SQL before running a query.",
            )));
            return;
        }
        self.record_query_history(widgets, &sql);

        let reload_schema_on_success = db::query::changes_schema(&sql);
        let id = self.allocate_query_id();
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        if let Some(tab) = self.query_tab_mut(tab_id) {
            tab.query_state = QueryState::Running;
            tab.editor.emit(SqlEditorMsg::SetRunning(true));
            tab.results.emit(QueryResultsMsg::Loading);
            tab.active_query = Some(RunningQuery { id, abort_handle });
        }

        sender.oneshot_command(async move {
            let query = async move {
                db::query::execute(&pool, &sql)
                    .await
                    .map_err(|error| error.to_string())
            };

            match Abortable::new(query, abort_registration).await {
                Ok(result) => WindowContentCommandOutput::QueryExecuted {
                    tab_id,
                    id,
                    reload_schema_on_success,
                    result,
                },
                Err(_) => WindowContentCommandOutput::QueryCancelled { tab_id, id },
            }
        });
    }

    fn cancel_query(&mut self, tab_id: u64) {
        let Some(tab) = self.query_tab_mut(tab_id) else {
            return;
        };

        let Some(active_query) = tab.active_query.take() else {
            return;
        };

        active_query.abort_handle.abort();
        tab.query_state = QueryState::Idle;
        tab.editor.emit(SqlEditorMsg::SetRunning(false));
        tab.results.emit(QueryResultsMsg::Cancelled);
    }

    fn cancel_all_queries(&mut self) {
        for tab in &mut self.query_tabs {
            if let Some(active_query) = tab.active_query.take() {
                active_query.abort_handle.abort();
                tab.query_state = QueryState::Idle;
                tab.editor.emit(SqlEditorMsg::SetRunning(false));
                tab.results.emit(QueryResultsMsg::Cancelled);
            }
        }
    }

    fn allocate_query_id(&mut self) -> u64 {
        let id = self.next_query_id;
        self.next_query_id = self.next_query_id.wrapping_add(1);
        id
    }

    fn allocate_schema_request_id(&mut self) -> u64 {
        let id = self.next_schema_request_id;
        self.next_schema_request_id = self.next_schema_request_id.wrapping_add(1);
        id
    }

    fn advance_table_script_generation(&mut self) {
        self.table_script_generation = self.table_script_generation.wrapping_add(1);
    }

    fn is_active_query(&self, tab_id: u64, id: u64) -> bool {
        self.query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.active_query.as_ref())
            .as_ref()
            .is_some_and(|query| query.id == id)
    }
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
