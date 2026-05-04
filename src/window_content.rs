use futures_util::future::{AbortHandle, Abortable};
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::models::connection::SavedConnection;
use crate::models::database_object::DatabaseObject;
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
    editor: Controller<SqlEditor>,
    results: Controller<QueryResults>,
    editor_buffer: sourceview5::Buffer,
    query_state: QueryState,
    active_schema_request_id: Option<u64>,
    next_schema_request_id: u64,
    active_query: Option<RunningQuery>,
    next_query_id: u64,
    workspace_navigation: WorkspaceNavigation,
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
    RunQuery,
    FocusEditor,
    ToggleSidebar,
    ConnectionDialogOutput(ConnectionDialogOutput),
    StartScreenOutput(StartScreenOutput),
    SidebarOutput(ObjectSidebarOutput),
    EditorOutput(SqlEditorOutput),
}

#[derive(Debug)]
pub enum WindowContentCommandOutput {
    SchemaLoaded {
        id: u64,
        result: Result<Vec<DatabaseObject>, String>,
    },
    QueryExecuted {
        id: u64,
        result: Result<QueryExecutionResult, String>,
    },
    QueryCancelled {
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

                    #[wrap(Some)]
                    set_title_widget = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        gtk::Label {
                            set_label: &gettext("Codd"),
                            add_css_class: "title-4",
                        },

                        gtk::Label {
                            add_css_class: "caption",
                            add_css_class: "dim-label",
                            #[watch]
                            set_label: &model.window_subtitle,
                            #[watch]
                            set_visible: !model.window_subtitle.is_empty(),
                        },
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
                        set_content = &adw::NavigationPage::builder()
                            .title(gettext("Workspace"))
                            .child(&gtk::Paned::builder()
                                .orientation(gtk::Orientation::Vertical)
                                .start_child(model.editor.widget())
                                .end_child(model.results.widget())
                                .resize_start_child(true)
                                .shrink_start_child(false)
                                .position(460)
                                .build())
                            .build(),
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
        let editor_buffer = sourceview5::Buffer::new(None);
        let editor = SqlEditor::builder()
            .launch(editor_buffer.clone())
            .forward(sender.input_sender(), WindowContentMsg::EditorOutput);
        let results = QueryResults::builder().launch(()).detach();

        let model = WindowContent {
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
            editor,
            results,
            editor_buffer,
            query_state: QueryState::Idle,
            active_schema_request_id: None,
            next_schema_request_id: 0,
            active_query: None,
            next_query_id: 0,
            workspace_navigation: WorkspaceNavigation::Wide,
        };
        let widgets = view_output!();

        widgets.content_stack.set_visible_child_name("start");
        let workspace_split_view = workspace_split_view(&widgets);
        workspace_split_view.set_show_content(true);

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
            WindowContentMsg::RunQuery
            | WindowContentMsg::EditorOutput(SqlEditorOutput::RunRequested) => {
                self.run_query(widgets, &sender, QueryHistoryMode::Save);
            }
            WindowContentMsg::EditorOutput(SqlEditorOutput::CancelRequested) => {
                self.cancel_query();
            }
            WindowContentMsg::EditorOutput(SqlEditorOutput::HistorySelected(sql)) => {
                self.editor_buffer.set_text(&sql);
                self.editor.emit(SqlEditorMsg::Focus);
            }
            WindowContentMsg::EditorOutput(SqlEditorOutput::ClearHistoryRequested) => {
                self.clear_query_history(widgets);
            }
            WindowContentMsg::FocusEditor => {
                self.editor.emit(SqlEditorMsg::Focus);
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
                self.editor_buffer.set_text(&query);
                self.editor.emit(SqlEditorMsg::Focus);
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
            WindowContentCommandOutput::QueryExecuted { id, result } => {
                self.handle_query_executed(id, result);
            }
            WindowContentCommandOutput::QueryCancelled { id } => self.handle_query_cancelled(id),
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
        self.window_subtitle.clear();
        self.active_pool = None;
        self.cancel_query();
        self.active_schema_request_id = None;
        self.state.active_connection = None;
        self.state.objects.clear();
        self.state.query_result = None;
        self.editor_buffer.set_text("");
        self.editor.emit(SqlEditorMsg::SetHistory(Vec::new()));
        self.results.emit(QueryResultsMsg::Clear);
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
        self.state.active_connection = Some(connection.clone());
        self.window_subtitle = format!(
            "{}@{}:{} / {}",
            connection.username, connection.host, connection.port, connection.database
        );
        self.active_pool = Some(pool.clone());
        self.connection_dialog = None;
        self.visible_page = VisiblePage::Workspace;
        self.editor_buffer.set_text("");
        self.load_query_history(connection);
        self.results.emit(QueryResultsMsg::Clear);
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

    fn handle_query_executed(&mut self, id: u64, result: Result<QueryExecutionResult, String>) {
        if !self.is_active_query(id) {
            return;
        }

        self.active_query = None;
        self.query_state = QueryState::Idle;
        self.editor.emit(SqlEditorMsg::SetRunning(false));

        match result {
            Ok(result) => {
                self.state.query_result = match &result {
                    QueryExecutionResult::Rows(rows) => Some(rows.clone()),
                    QueryExecutionResult::AffectedRows(_) => None,
                };
                self.results.emit(QueryResultsMsg::ShowResult(result));
            }
            Err(error) => {
                self.state.query_result = None;
                self.results.emit(QueryResultsMsg::ShowError(format!(
                    "{}: {error}",
                    gettext("Query failed")
                )));
            }
        }
    }

    fn handle_query_cancelled(&mut self, id: u64) {
        if !self.is_active_query(id) {
            return;
        }

        self.active_query = None;
        self.query_state = QueryState::Idle;
        self.editor.emit(SqlEditorMsg::SetRunning(false));
        self.results.emit(QueryResultsMsg::Cancelled);
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
        if self.query_state == QueryState::Running {
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

        let sql = self.editor_sql();
        if sql.trim().is_empty() {
            widgets.toast_overlay.add_toast(adw::Toast::new(&gettext(
                "Enter SQL before running a query.",
            )));
            return;
        }
        if history_mode == QueryHistoryMode::Save {
            self.record_query_history(widgets, &sql);
        }

        self.query_state = QueryState::Running;
        self.editor.emit(SqlEditorMsg::SetRunning(true));
        self.results.emit(QueryResultsMsg::Loading);

        let id = self.allocate_query_id();
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.active_query = Some(RunningQuery { id, abort_handle });

        sender.oneshot_command(async move {
            let query = async move {
                db::query::execute(&pool, &sql)
                    .await
                    .map_err(|error| error.to_string())
            };

            match Abortable::new(query, abort_registration).await {
                Ok(result) => WindowContentCommandOutput::QueryExecuted { id, result },
                Err(_) => WindowContentCommandOutput::QueryCancelled { id },
            }
        });
    }

    fn cancel_query(&mut self) {
        let Some(active_query) = self.active_query.take() else {
            return;
        };

        active_query.abort_handle.abort();
        self.query_state = QueryState::Idle;
        self.editor.emit(SqlEditorMsg::SetRunning(false));
        self.results.emit(QueryResultsMsg::Cancelled);
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

    fn is_active_query(&self, id: u64) -> bool {
        self.active_query
            .as_ref()
            .is_some_and(|query| query.id == id)
    }

    fn editor_sql(&self) -> String {
        self.editor_buffer
            .text(
                &self.editor_buffer.start_iter(),
                &self.editor_buffer.end_iter(),
                false,
            )
            .to_string()
    }

    fn load_query_history(&self, connection: &SavedConnection) {
        self.editor.emit(SqlEditorMsg::SetHistory(
            query_history_store::load_for_connection(&connection.id),
        ));
    }

    fn record_query_history(&self, widgets: &WindowContentWidgets, sql: &str) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        match query_history_store::record_query(&connection.id, sql) {
            Ok(history) => {
                self.editor.emit(SqlEditorMsg::SetHistory(history));
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Saving query history failed")
                )));
            }
        }
    }

    fn clear_query_history(&self, widgets: &WindowContentWidgets) {
        let Some(connection) = self.state.active_connection.as_ref() else {
            return;
        };

        match query_history_store::clear_connection(&connection.id) {
            Ok(history) => {
                self.editor.emit(SqlEditorMsg::SetHistory(history));
            }
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {error}",
                    gettext("Clearing query history failed")
                )));
            }
        }
    }
}
