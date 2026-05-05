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
use crate::models::connection::SavedConnection;
use crate::models::database_object::DatabaseObject;
use crate::models::query_history::QueryHistoryEntry;
use crate::models::query_result::QueryExecutionResult;
use crate::settings;
use crate::state::{app_state::AppState, connection_store, query_history_store};
use crate::ui::components::{
    connection_dialog::{ConnectionDialog, ConnectionDialogInit, ConnectionDialogOutput},
    editor::{SqlEditor, SqlEditorMsg, SqlEditorOutput},
    results::{QueryResults, QueryResultsMsg},
    sidebar::{ObjectSidebar, ObjectSidebarMsg, ObjectSidebarOutput},
    start_screen::{StartScreen, StartScreenMsg, StartScreenOutput},
};

pub struct WindowContent {
    state: AppState,
    active_pool: Option<PgPool>,
    connection_dialog: Option<Controller<ConnectionDialog>>,
    window_subtitle: String,
    visible_page: VisiblePage,
    start_screen: Controller<StartScreen>,
    sidebar: Controller<ObjectSidebar>,
    query_tabs: Vec<QueryTab>,
    query_history: Vec<QueryHistoryEntry>,
    active_query_tab_id: u64,
    next_query_tab_id: u64,
    menu_button: gtk::MenuButton,
    active_schema_request_id: Option<u64>,
    next_schema_request_id: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryHistoryMode {
    Save,
    Skip,
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
    CloseQueryTab(u64),
    QueryTabTitleChanged(u64),
    RunQuery,
    FocusEditor,
    ToggleSidebar,
    ConnectionDialogOutput(ConnectionDialogOutput),
    StartScreenOutput(StartScreenOutput),
    SidebarOutput(ObjectSidebarOutput),
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
    QueryExecuted {
        tab_id: u64,
        id: u64,
        result: Result<QueryExecutionResult, String>,
    },
    QueryCancelled {
        tab_id: u64,
        id: u64,
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
                    set_title_widget = &adw::WindowTitle {
                        set_title: &gettext("Codd"),
                        #[watch]
                        set_subtitle: &model.window_subtitle,
                    },
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
                                    set_visible: model.shows_workspace() && model.has_multiple_query_tabs(),

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

        let mut model = WindowContent {
            state: AppState {
                connections,
                ..AppState::default()
            },
            active_pool: None,
            connection_dialog: None,
            window_subtitle: String::new(),
            visible_page: VisiblePage::Start,
            start_screen,
            sidebar,
            query_tabs: Vec::new(),
            query_history: Vec::new(),
            active_query_tab_id: 0,
            next_query_tab_id: 0,
            menu_button: gtk::MenuButton::new(),
            active_schema_request_id: None,
            next_schema_request_id: 0,
            next_query_id: 0,
            workspace_navigation: WorkspaceNavigation::Wide,
        };
        let widgets = view_output!();

        model.add_query_tab(&widgets, &sender);
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
                }
            });

        let s = sender.clone();
        widgets.query_tab_view.connect_close_page(move |_, page| {
            if let Some(tab_id) = query_tab_id_from_widget(&page.child()) {
                s.input(WindowContentMsg::CloseQueryTab(tab_id));
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
                }
            }
            WindowContentMsg::CloseQueryTab(tab_id) => {
                self.close_query_tab(tab_id, widgets);
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
                self.run_query(widgets, &sender, QueryHistoryMode::Save);
            }
            WindowContentMsg::EditorOutput {
                tab_id,
                output: SqlEditorOutput::RunRequested,
            } => {
                self.run_query_for_tab(tab_id, widgets, &sender, QueryHistoryMode::Save);
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
                if let Some(tab) = self.active_query_tab_mut() {
                    tab.editor.emit(SqlEditorMsg::Focus);
                }
            }
            WindowContentMsg::ToggleSidebar => self.toggle_sidebar(widgets, root),
            WindowContentMsg::ConnectionDialogOutput(ConnectionDialogOutput::Connected {
                connection,
                pool,
            }) => self.handle_connected(&connection, pool, widgets, &sender, root),
            WindowContentMsg::ConnectionDialogOutput(ConnectionDialogOutput::Dismissed) => {
                self.connection_dialog = None;
            }
            WindowContentMsg::SidebarOutput(ObjectSidebarOutput::PrepareQuery(query)) => {
                if let Some(tab) = self.active_query_tab_mut() {
                    tab.editor_buffer.set_text(&query);
                    tab.editor.emit(SqlEditorMsg::Focus);
                }
                workspace_split_view(widgets).set_show_content(true);
                self.workspace_navigation = if workspace_split_view(widgets).is_collapsed() {
                    WorkspaceNavigation::Content
                } else {
                    WorkspaceNavigation::Wide
                };
                self.run_query(widgets, &sender, QueryHistoryMode::Skip);
            }
        }

        self.update_view(widgets, sender);
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        _sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            WindowContentCommandOutput::SchemaLoaded { id, result } => {
                self.handle_schema_loaded(id, result);
            }
            WindowContentCommandOutput::QueryExecuted { tab_id, id, result } => {
                self.handle_query_executed(tab_id, id, result);
            }
            WindowContentCommandOutput::QueryCancelled { tab_id, id } => {
                self.handle_query_cancelled(tab_id, id);
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

fn query_tab_id_from_widget(widget: &gtk::Widget) -> Option<u64> {
    widget
        .widget_name()
        .strip_prefix("query-tab-")
        .and_then(|id| id.parse().ok())
}

fn query_tab_title(sql: &str) -> String {
    let preview = sql
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .flat_map(|line| line.split_whitespace())
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");

    if preview.is_empty() {
        gettext("Query")
    } else {
        truncate_for_tab_title(&preview)
    }
}

fn truncate_for_tab_title(value: &str) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(28).collect();

    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn icon_label_widget(icon_name: &str, label: &str) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let image = gtk::Image::from_icon_name(icon_name);
    container.append(&image);

    let label = gtk::Label::new(Some(label));
    container.append(&label);
    container
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
    fn add_query_tab(&mut self, widgets: &WindowContentWidgets, sender: &ComponentSender<Self>) {
        let id = self.next_query_tab_id;
        self.next_query_tab_id = self.next_query_tab_id.wrapping_add(1);

        let editor_buffer = sourceview5::Buffer::new(None);
        let s = sender.clone();
        editor_buffer.connect_changed(move |_| {
            s.input(WindowContentMsg::QueryTabTitleChanged(id));
        });

        let editor = SqlEditor::builder()
            .launch(editor_buffer.clone())
            .forward(sender.input_sender(), move |output| {
                WindowContentMsg::EditorOutput { tab_id: id, output }
            });

        let results = QueryResults::builder().launch(()).detach();

        let widget = gtk::Paned::builder()
            .orientation(gtk::Orientation::Vertical)
            .start_child(editor.widget())
            .end_child(results.widget())
            .resize_start_child(true)
            .shrink_start_child(false)
            .position(460)
            .build();

        widget.set_widget_name(&format!("query-tab-{id}"));

        let title = query_tab_title("");
        let page = widgets.query_tab_view.append(&widget);
        page.set_title(&title);
        page.set_tooltip(&title);
        widgets.query_tab_view.set_selected_page(&page);

        self.active_query_tab_id = id;
        self.query_tabs.push(QueryTab {
            id,
            page,
            editor,
            results,
            editor_buffer,
            query_state: QueryState::Idle,
            active_query: None,
        });

        if let Some(tab) = self.query_tabs.last() {
            tab.editor
                .emit(SqlEditorMsg::SetHistory(self.query_history.clone()));
        }
    }

    fn add_query_tab_if_workspace_visible(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if !self.shows_workspace() {
            return;
        }

        self.add_query_tab(widgets, sender);
    }

    fn close_query_tab(&mut self, tab_id: u64, widgets: &WindowContentWidgets) {
        if self.query_tabs.len() <= 1 {
            return;
        }

        let Some(index) = self.query_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };

        if let Some(active_query) = self.query_tabs[index].active_query.take() {
            active_query.abort_handle.abort();
        }

        let removed_was_active = self.active_query_tab_id == tab_id;
        let removed = self.query_tabs.remove(index);
        widgets
            .query_tab_view
            .close_page_finish(&removed.page, true);

        if removed_was_active {
            let next_index = index.min(self.query_tabs.len() - 1);
            let next_tab = &self.query_tabs[next_index];
            self.active_query_tab_id = next_tab.id;
            widgets.query_tab_view.set_selected_page(&next_tab.page);
        }
    }

    fn active_query_tab(&self) -> Option<&QueryTab> {
        self.query_tabs
            .iter()
            .find(|tab| tab.id == self.active_query_tab_id)
    }

    fn active_query_tab_mut(&mut self) -> Option<&mut QueryTab> {
        let active_query_tab_id = self.active_query_tab_id;
        self.query_tabs
            .iter_mut()
            .find(|tab| tab.id == active_query_tab_id)
    }

    fn query_tab_mut(&mut self, tab_id: u64) -> Option<&mut QueryTab> {
        self.query_tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    fn update_query_tab_title(&mut self, tab_id: u64) {
        let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };

        let title = query_tab_title(&self.query_tab_sql(tab_id).unwrap_or_default());
        tab.page.set_title(&title);
        tab.page.set_tooltip(&title);
    }

    fn has_multiple_query_tabs(&self) -> bool {
        self.query_tabs.len() > 1
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
        self.visible_page = VisiblePage::Start;
        self.window_subtitle.clear();
        self.active_pool = None;
        self.cancel_all_queries();
        self.active_schema_request_id = None;
        self.state.active_connection = None;
        self.state.objects.clear();
        self.query_history.clear();

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

    fn handle_connected(
        &mut self,
        connection: &SavedConnection,
        pool: PgPool,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
        root: &adw::ToolbarView,
    ) {
        self.cancel_all_queries();

        self.state.active_connection = Some(connection.clone());
        self.window_subtitle = format!(
            "{}@{}:{} / {}",
            connection.username, connection.host, connection.port, connection.database
        );
        self.active_pool = Some(pool.clone());
        self.connection_dialog = None;
        self.visible_page = VisiblePage::Workspace;

        for tab in &self.query_tabs {
            tab.editor_buffer.set_text("");
            tab.results.emit(QueryResultsMsg::Clear);
        }
        self.load_query_history(connection);
        self.sidebar.emit(ObjectSidebarMsg::Loading);

        self.show_workspace(widgets, root, connection);

        match connection_store::save_connection(connection) {
            Ok(connections) => {
                self.state.connections.clone_from(&connections);
                self.start_screen
                    .emit(StartScreenMsg::SetConnections(connections));
                widgets
                    .toast_overlay
                    .add_toast(adw::Toast::new(&gettext("Connected to PostgreSQL.")));
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

    fn handle_query_executed(
        &mut self,
        tab_id: u64,
        id: u64,
        result: Result<QueryExecutionResult, String>,
    ) {
        if !self.is_active_query(tab_id, id) {
            return;
        }

        let Some(tab) = self.query_tab_mut(tab_id) else {
            return;
        };

        tab.active_query = None;
        tab.query_state = QueryState::Idle;
        tab.editor.emit(SqlEditorMsg::SetRunning(false));

        match result {
            Ok(result) => {
                tab.results.emit(QueryResultsMsg::ShowResult(result));
            }
            Err(error) => {
                tab.results.emit(QueryResultsMsg::ShowError(format!(
                    "{}: {error}",
                    gettext("Query failed")
                )));
            }
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

    fn run_query(
        &mut self,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
        history_mode: QueryHistoryMode,
    ) {
        let Some(tab_id) = self.active_query_tab().map(|tab| tab.id) else {
            self.add_query_tab(widgets, sender);
            return;
        };

        self.run_query_for_tab(tab_id, widgets, sender, history_mode);
    }

    fn run_query_for_tab(
        &mut self,
        tab_id: u64,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
        history_mode: QueryHistoryMode,
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

        let sql = self.query_tab_sql(tab_id).unwrap_or_default();
        if sql.trim().is_empty() {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Enter SQL before running a query.",
            )));
            return;
        }
        if history_mode == QueryHistoryMode::Save {
            self.record_query_history(widgets, &sql);
        }

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
                Ok(result) => WindowContentCommandOutput::QueryExecuted { tab_id, id, result },
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

    fn is_active_query(&self, tab_id: u64, id: u64) -> bool {
        self.query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.active_query.as_ref())
            .as_ref()
            .is_some_and(|query| query.id == id)
    }

    fn query_tab_sql(&self, tab_id: u64) -> Option<String> {
        let buffer = &self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)?
            .editor_buffer;

        Some(
            buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string(),
        )
    }

    fn load_query_history(&mut self, connection: &SavedConnection) {
        self.set_query_history(query_history_store::load_for_connection(&connection.id));
    }

    fn record_query_history(&mut self, widgets: &WindowContentWidgets, sql: &str) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        match query_history_store::record_query(&connection.id, sql) {
            Ok(history) => {
                self.set_query_history(history);
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Saving query history failed")
                )));
            }
        }
    }

    fn clear_query_history(&mut self, widgets: &WindowContentWidgets) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        match query_history_store::clear_connection(&connection.id) {
            Ok(history) => {
                self.set_query_history(history);
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Clearing query history failed")
                )));
            }
        }
    }

    fn set_query_history(&mut self, history: Vec<QueryHistoryEntry>) {
        self.query_history = history;

        for tab in &self.query_tabs {
            tab.editor
                .emit(SqlEditorMsg::SetHistory(self.query_history.clone()));
        }
    }
}
