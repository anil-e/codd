use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::gio;
use relm4::prelude::*;

use crate::models::database_object::DatabaseObject;
use crate::models::query_result::{
    DEFAULT_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT,
};
use crate::models::session::{
    SavedSession, SavedSessionObject, SavedSessionTab, SavedSessionTabId,
};
use crate::models::structure_action::{StructureActionKind, StructureActionTarget};
use crate::ui::components::{
    editor::{SqlEditor, SqlEditorMsg},
    results::QueryResults,
    sidebar::ObjectSidebarMsg,
    table_view::{TableView, TableViewMsg},
};

use super::{
    BrowseTab, QueryResultsMsg, QueryState, QueryTab, StructureActionScope, WindowContent,
    WindowContentMsg, WindowContentWidgets, WorkspaceNavigation, workspace_split_view,
};

thread_local! {
    static SESSION_TAB_IDS: RefCell<SessionTabIds> = RefCell::new(SessionTabIds::default());
}

#[derive(Default)]
struct SessionTabIds {
    next: u64,
    reserved: HashSet<u64>,
}

impl SessionTabIds {
    fn reserve(&mut self, id: u64) {
        self.reserved.insert(id);
        self.next = self.next.max(id.saturating_add(1));
    }

    fn allocate(&mut self) -> u64 {
        while self.reserved.contains(&self.next) {
            self.next = self.next.wrapping_add(1);
        }
        let id = self.next;
        self.reserve(id);
        id
    }
}

pub(super) fn reserve_query_session_id(id: u64) {
    SESSION_TAB_IDS.with(|ids| ids.borrow_mut().reserve(id));
}

pub(super) fn selected_query_tab_id(widgets: &WindowContentWidgets) -> Option<u64> {
    widgets
        .query_tab_view
        .selected_page()
        .and_then(|page| query_tab_id_from_widget(&page.child()))
}

pub(super) fn selected_browse_tab_id(widgets: &WindowContentWidgets) -> Option<u64> {
    widgets
        .query_tab_view
        .selected_page()
        .and_then(|page| browse_tab_id_from_widget(&page.child()))
}

fn close_overlay_sidebar_if_needed(widgets: &WindowContentWidgets) {
    let split_view = workspace_split_view(widgets);

    if split_view.is_collapsed() {
        split_view.set_show_sidebar(false);
    }
}

pub(super) fn setup_tab_context_menu(
    tab_view: &adw::TabView,
    sender: &ComponentSender<WindowContent>,
) {
    let menu = gio::Menu::new();
    menu.append(
        Some(&gettext("Open in New Tab")),
        Some("tab.open-in-new-tab"),
    );

    menu.append(Some(&gettext("Close")), Some("tab.close"));
    menu.append(
        Some(&gettext("Close Other Tabs")),
        Some("tab.close-other-tabs"),
    );

    menu.append(Some(&gettext("Close All")), Some("tab.close-all"));
    menu.append(Some(&gettext("Save")), Some("tab.save"));
    menu.append(Some(&gettext("Save As")), Some("tab.save-as"));
    menu.append(Some(&gettext("Reload")), Some("tab.reload"));
    tab_view.set_menu_model(Some(&menu));

    let current_tab = Rc::new(RefCell::new(None::<String>));
    let action_group = gio::SimpleActionGroup::new();
    let mut file_actions = Vec::new();
    for name in ["save", "save-as", "reload"] {
        let action = gio::SimpleAction::new(name, None);
        let current_tab = current_tab.clone();
        let sender = sender.input_sender().clone();
        action.connect_activate(move |_, _| {
            let target = current_tab.borrow().clone();
            let msg = if name == "reload" {
                super::documents::DocumentMsg::ReloadTab(target)
            } else {
                super::documents::DocumentMsg::SaveTab(target, name == "save-as")
            };
            let _ = sender.send(WindowContentMsg::Document(msg));
        });
        action_group.add_action(&action);
        file_actions.push(action);
    }

    let close_action = gio::SimpleAction::new("close", None);

    close_action.connect_activate({
        let sender = sender.input_sender().clone();
        let current_tab = current_tab.clone();

        move |_, _| {
            let _ = sender.send(WindowContentMsg::CloseTabFromMenu(
                current_tab.borrow().clone(),
            ));
        }
    });

    action_group.add_action(&close_action);

    let open_in_new_tab_action = gio::SimpleAction::new("open-in-new-tab", None);

    open_in_new_tab_action.connect_activate({
        let sender = sender.input_sender().clone();
        let current_tab = current_tab.clone();

        move |_, _| {
            let _ = sender.send(WindowContentMsg::OpenBrowseTabFromMenu(
                current_tab.borrow().clone(),
            ));
        }
    });

    action_group.add_action(&open_in_new_tab_action);

    let close_other_tabs_action = gio::SimpleAction::new("close-other-tabs", None);

    close_other_tabs_action.connect_activate({
        let sender = sender.input_sender().clone();
        let current_tab = current_tab.clone();

        move |_, _| {
            let _ = sender.send(WindowContentMsg::CloseOtherTabsFromMenu(
                current_tab.borrow().clone(),
            ));
        }
    });

    action_group.add_action(&close_other_tabs_action);

    let close_all_action = gio::SimpleAction::new("close-all", None);
    close_all_action.connect_activate({
        let sender = sender.input_sender().clone();

        move |_, _| {
            let _ = sender.send(WindowContentMsg::CloseAllTabs);
        }
    });

    action_group.add_action(&close_all_action);

    tab_view.connect_setup_menu({
        let close_action = close_action.clone();
        let open_in_new_tab_action = open_in_new_tab_action.clone();
        let close_all_action = close_all_action.clone();
        let close_other_tabs_action = close_other_tabs_action.clone();
        let current_tab = current_tab.clone();

        move |tab_view, page| {
            let widget_name = page.map(|page| page.child().widget_name().to_string());
            let has_target_tab = widget_name.is_some();
            let can_close_multiple = tab_view.n_pages() > 1;

            *current_tab.borrow_mut() = widget_name;
            let is_query = current_tab
                .borrow()
                .as_deref()
                .is_some_and(|name| name.starts_with("query-tab-"));
            for action in &file_actions {
                action.set_enabled(is_query);
            }

            let can_open_in_new_tab = current_tab
                .borrow()
                .as_deref()
                .and_then(|name| tab_id_from_widget_name(Some(name), "browse-tab-"))
                .is_some();

            open_in_new_tab_action.set_enabled(can_open_in_new_tab);
            close_action.set_enabled(has_target_tab);
            close_other_tabs_action.set_enabled(has_target_tab && can_close_multiple);
            close_all_action.set_enabled(can_close_multiple);
        }
    });

    tab_view.connect_realize(move |tab_view| {
        if let Some(window) = tab_view.root().and_downcast::<gtk::Window>() {
            window.insert_action_group("tab", Some(&action_group));
        }
    });
}

impl WindowContent {
    pub(super) fn add_query_tab(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        self.add_query_tab_with_row_limit(DEFAULT_QUERY_RESULT_ROW_LIMIT, widgets, sender);
    }

    fn add_query_tab_with_row_limit(
        &mut self,
        row_limit: usize,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let id = self.next_query_tab_id;
        self.add_query_tab_with_id(id, row_limit, widgets, sender);
    }

    fn add_query_tab_with_id(
        &mut self,
        id: u64,
        row_limit: usize,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        self.next_query_tab_id = self.next_query_tab_id.max(id.wrapping_add(1));
        let row_limit = row_limit.clamp(MIN_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT);

        let editor_buffer = sourceview5::Buffer::new(None);
        let s = sender.input_sender().clone();
        editor_buffer.connect_changed(move |_| {
            let _ = s.send(WindowContentMsg::QueryTabTitleChanged(id));
        });

        let editor = SqlEditor::builder()
            .launch(editor_buffer.clone())
            .forward(sender.input_sender(), move |output| {
                WindowContentMsg::EditorOutput { tab_id: id, output }
            });

        let results = QueryResults::builder()
            .launch(row_limit)
            .forward(sender.input_sender(), move |output| {
                WindowContentMsg::ResultsOutput { tab_id: id, output }
            });

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
            session_id: SESSION_TAB_IDS.with(|ids| ids.borrow_mut().allocate()),
            page,
            editor,
            results,
            editor_buffer,
            row_limit,
            query_state: QueryState::Idle,
            active_query: None,
            document: None,
        });

        if let Some(tab) = self.query_tabs.last() {
            tab.editor
                .emit(SqlEditorMsg::SetHistory(self.query_history.clone()));
            tab.editor.emit(SqlEditorMsg::SetCompletionCatalog(
                self.completion_catalog.clone(),
            ));
        }
    }

    pub(super) fn add_query_tab_with_sql(
        &mut self,
        sql: String,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        self.add_query_tab(widgets, sender);

        if let Some(tab) = self.active_query_tab_mut() {
            tab.editor_buffer.set_text(&sql);
            tab.editor.emit(SqlEditorMsg::Focus);
        }

        self.update_query_tab_title(self.active_query_tab_id);

        close_overlay_sidebar_if_needed(widgets);
        self.workspace_navigation = WorkspaceNavigation::from_sidebar_visibility(
            workspace_split_view(widgets).shows_sidebar(),
        );
    }

    pub(super) fn build_saved_session(
        &self,
        connection_id: String,
        database: String,
        widgets: &WindowContentWidgets,
    ) -> SavedSession {
        let mut tabs = Vec::with_capacity(self.tab_count());

        for index in 0..widgets.query_tab_view.n_pages() {
            let page = widgets.query_tab_view.nth_page(index);

            if let Some(tab_id) = query_tab_id_from_widget(&page.child())
                && let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == tab_id)
                && tab.document.is_none()
            {
                tabs.push(SavedSessionTab::Query {
                    id: tab.session_id,
                    sql: self.query_tab_sql(tab.id).unwrap_or_default(),
                    row_limit: tab.row_limit,
                });
            } else if let Some(tab_id) = browse_tab_id_from_widget(&page.child())
                && let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id)
            {
                tabs.push(SavedSessionTab::Browse {
                    id: tab.id,
                    object: SavedSessionObject::from_database_object(&tab.object),
                });
            }
        }

        SavedSession {
            connection_id,
            database,
            active_tab: self.active_session_tab_id(widgets),
            tabs,
        }
    }

    pub(super) fn restore_session(
        &mut self,
        session: Option<SavedSession>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(session) = &session {
            for tab in &session.tabs {
                if let SavedSessionTab::Query { id, .. } = tab {
                    reserve_query_session_id(*id);
                }
            }
        }
        let tab_view_signals_blocked = self.tab_view_signals_blocked.clone();
        let active_tab = with_tab_view_signals_blocked(&tab_view_signals_blocked, || {
            self.clear_all_tabs(widgets);

            let session = session?;

            let saved_active_tab = session.active_tab;
            let mut active_tab = saved_active_tab;

            for tab in session.tabs {
                match tab {
                    SavedSessionTab::Query { id, sql, row_limit } => {
                        self.add_query_tab_with_row_limit(row_limit, widgets, sender);
                        if saved_active_tab == Some(SavedSessionTabId::Query(id)) {
                            active_tab = Some(SavedSessionTabId::Query(self.active_query_tab_id));
                        }

                        if let Some(tab) = self.active_query_tab_mut() {
                            tab.session_id = id;
                            tab.editor_buffer.set_text(&sql);
                            tab.results.emit(QueryResultsMsg::Clear);
                        }

                        self.update_query_tab_title(self.active_query_tab_id);
                    }
                    SavedSessionTab::Browse { id, object } => {
                        self.add_browse_tab_with_id(
                            id,
                            object.to_database_object(),
                            widgets,
                            false,
                            sender,
                        );
                    }
                }
            }

            active_tab
        });

        self.select_session_tab(active_tab, widgets, sender);
        self.load_selected_browse_tab_if_needed(widgets, sender);

        if self.tab_count() == 0 {
            self.add_query_tab(widgets, sender);
        }

        self.sync_sidebar_selection(widgets);
    }

    pub(super) fn add_query_tab_if_workspace_visible(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if !self.shows_workspace() {
            return;
        }

        self.add_query_tab(widgets, sender);
    }

    pub(super) fn close_query_tab(&mut self, tab_id: u64, widgets: &WindowContentWidgets) {
        if self.tab_count() <= 1 {
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
        self.close_pages_immediately(widgets, [removed.page]);

        if removed_was_active {
            if self.query_tabs.is_empty() {
                self.active_query_tab_id = 0;
            } else {
                let next_index = index.min(self.query_tabs.len() - 1);
                let next_tab = &self.query_tabs[next_index];
                self.active_query_tab_id = next_tab.id;
                widgets.query_tab_view.set_selected_page(&next_tab.page);
            }
        }
    }

    pub(super) fn close_browse_tab(&mut self, tab_id: u64, widgets: &WindowContentWidgets) {
        if self.tab_count() <= 1 {
            return;
        }

        let Some(index) = self.browse_tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };

        let removed = self.browse_tabs.remove(index);
        self.close_pages_immediately(widgets, [removed.page]);
    }

    pub(super) fn close_active_tab(&mut self, widgets: &WindowContentWidgets) {
        if !self.shows_workspace() {
            return;
        }

        let Some(page) = widgets.query_tab_view.selected_page() else {
            return;
        };

        widgets.query_tab_view.close_page(&page);
    }

    pub(super) fn close_tab_from_widget_name(
        &mut self,
        widget_name: Option<&str>,
        widgets: &WindowContentWidgets,
    ) {
        if let Some(tab_id) = tab_id_from_widget_name(widget_name, "query-tab-")
            && let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == tab_id)
        {
            widgets.query_tab_view.close_page(&tab.page);
            return;
        }

        if let Some(tab_id) = tab_id_from_widget_name(widget_name, "browse-tab-")
            && let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id)
        {
            widgets.query_tab_view.close_page(&tab.page);
        }
    }

    pub(super) fn active_query_tab(&self) -> Option<&QueryTab> {
        self.query_tabs
            .iter()
            .find(|tab| tab.id == self.active_query_tab_id)
    }

    pub(super) fn active_query_tab_mut(&mut self) -> Option<&mut QueryTab> {
        let active_query_tab_id = self.active_query_tab_id;
        self.query_tabs
            .iter_mut()
            .find(|tab| tab.id == active_query_tab_id)
    }

    pub(super) fn query_tab_mut(&mut self, tab_id: u64) -> Option<&mut QueryTab> {
        self.query_tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    pub(super) fn update_query_tab_title(&mut self, tab_id: u64) {
        let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };

        if let Some(document) = &tab.document {
            let text = self.query_tab_sql(tab_id).unwrap_or_default();
            tab.page.set_title(&document.title());
            tab.page.set_tooltip(&document.tooltip());
            tab.page.set_needs_attention(document.is_dirty(&text));
            let icon = gio::ThemedIcon::new("edit-symbolic");
            tab.page
                .set_indicator_icon(document.is_dirty(&text).then_some(&icon));
            let warning = if document.file.is_none() {
                Some(gettext(
                    "This recovered script has no writable file target. Use Save As to keep it.",
                ))
            } else if document.external_changed {
                Some(gettext(
                    "The file changed on disk or is unavailable. Reload to read the latest version.",
                ))
            } else {
                None
            };
            tab.editor.emit(SqlEditorMsg::SetDocumentWarning(
                warning,
                document.file.is_some(),
            ));
            return;
        }

        let title = query_tab_title(&self.query_tab_sql(tab_id).unwrap_or_default());
        tab.page.set_title(&title);
        tab.page.set_tooltip(&title);
    }

    pub(super) fn has_multiple_tabs(&self) -> bool {
        self.tab_count() > 1 || self.query_tabs.iter().any(|tab| tab.document.is_some())
    }

    pub(super) fn tab_count(&self) -> usize {
        self.query_tabs.len() + self.browse_tabs.len()
    }

    pub(super) fn run_selected_query_tab(
        &mut self,
        widgets: &mut WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tab_id) = selected_query_tab_id(widgets) else {
            self.select_active_query_tab(widgets, sender);
            return;
        };

        self.active_query_tab_id = tab_id;

        self.run_query_for_tab(tab_id, widgets, sender);
    }

    pub(super) fn cancel_active_query(&mut self, widgets: &WindowContentWidgets) {
        let Some(tab_id) = selected_query_tab_id(widgets) else {
            return;
        };

        self.cancel_query(tab_id);
    }

    pub(super) fn refresh_active_browse_tab(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tab_id) = selected_browse_tab_id(widgets) else {
            return;
        };

        self.load_browse_tab_if_needed(tab_id, sender);

        if let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id)
            && let Some(view) = &tab.view
        {
            view.emit(TableViewMsg::Refresh);
        }
    }

    pub(super) fn query_tab_sql(&self, tab_id: u64) -> Option<String> {
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

    pub(super) fn query_tab_execution_sql(&self, tab_id: u64) -> Option<String> {
        let buffer = &self
            .query_tabs
            .iter()
            .find(|tab| tab.id == tab_id)?
            .editor_buffer;

        if let Some((start, end)) = buffer.selection_bounds() {
            return Some(buffer.text(&start, &end, false).to_string());
        }

        self.query_tab_sql(tab_id)
    }

    pub(super) fn open_table_browser(
        &mut self,
        object: DatabaseObject,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if let Some((tab_id, page, object)) = self
            .browse_tabs
            .iter()
            .find(|tab| tab.object == object)
            .map(|tab| (tab.id, tab.page.clone(), tab.object.clone()))
        {
            let was_loaded = self
                .browse_tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| tab.loaded);
            widgets.query_tab_view.set_selected_page(&page);
            self.load_browse_tab_if_needed(tab_id, sender);

            if was_loaded
                && let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id)
                && let Some(view) = &tab.view
            {
                view.emit(TableViewMsg::Refresh);
            }

            self.sidebar
                .emit(ObjectSidebarMsg::SetSelectedObject(Some(object)));
            close_overlay_sidebar_if_needed(widgets);
            self.workspace_navigation = WorkspaceNavigation::from_sidebar_visibility(
                workspace_split_view(widgets).shows_sidebar(),
            );
            return;
        }

        if self.active_pool.is_none() {
            return;
        };

        self.add_browse_tab(object, widgets, sender);
        close_overlay_sidebar_if_needed(widgets);
        self.workspace_navigation = WorkspaceNavigation::from_sidebar_visibility(
            workspace_split_view(widgets).shows_sidebar(),
        );
    }

    pub(super) fn add_browse_tab_from_widget_name(
        &mut self,
        widget_name: Option<&str>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tab_id) = tab_id_from_widget_name(widget_name, "browse-tab-") else {
            return;
        };

        let Some(object) = self
            .browse_tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.object.clone())
        else {
            return;
        };

        self.add_browse_tab(object, widgets, sender);
    }

    pub(super) fn add_browse_tab(
        &mut self,
        object: DatabaseObject,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let id = self.next_browse_tab_id;
        self.add_browse_tab_with_id(id, object, widgets, true, sender);
    }

    fn add_browse_tab_with_id(
        &mut self,
        id: u64,
        object: DatabaseObject,
        widgets: &WindowContentWidgets,
        load: bool,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pool) = self.active_pool.clone() else {
            return;
        };
        let Some((connection_id, database)) = self.active_database_scope() else {
            return;
        };
        let scope = StructureActionScope {
            connection_id,
            database,
        };

        self.next_browse_tab_id = self.next_browse_tab_id.max(id.wrapping_add(1));
        let stack = gtk::Stack::new();
        stack.set_widget_name(&format!("browse-tab-{id}"));
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(
            &adw::StatusPage::builder()
                .icon_name("table-symbolic")
                .title(gettext("Table"))
                .description(gettext("Select the tab to load this table."))
                .build(),
            Some("placeholder"),
        );

        let title = object.name.clone();
        let page = widgets.query_tab_view.append(&stack);
        page.set_title(&title);
        page.set_tooltip(&format!("{}.{}", object.schema, object.name));
        if load {
            widgets.query_tab_view.set_selected_page(&page);
        }

        self.browse_tabs.push(BrowseTab {
            id,
            page,
            scope,
            pool,
            object: object.clone(),
            stack,
            view: None,
            loaded: false,
            pending_edits: false,
        });

        if load {
            self.load_browse_tab_if_needed(id, sender);
        }

        if load && let Some(tab) = self.browse_tabs.last() {
            self.sidebar.emit(ObjectSidebarMsg::SetSelectedObject(Some(
                tab.object.clone(),
            )));
        }
    }

    pub(super) fn load_browse_tab_if_needed(
        &mut self,
        tab_id: u64,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tab) = self.browse_tabs.iter_mut().find(|tab| tab.id == tab_id) else {
            return;
        };

        if tab.loaded {
            return;
        }

        let view = TableView::builder()
            .launch(())
            .forward(sender.input_sender(), move |output| {
                WindowContentMsg::BrowseTabOutput { tab_id, output }
            });
        tab.stack.add_named(view.widget(), Some("content"));
        tab.stack.set_visible_child_name("content");
        view.emit(TableViewMsg::Open {
            pool: tab.pool.clone(),
            object: tab.object.clone(),
        });
        tab.view = Some(view);
        tab.loaded = true;
    }

    fn load_selected_browse_tab_if_needed(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(tab_id) = selected_browse_tab_id(widgets) {
            self.load_browse_tab_if_needed(tab_id, sender);
        }
    }

    pub(super) fn apply_renamed_object(
        &mut self,
        object: &DatabaseObject,
        new_name: &str,
        widgets: &WindowContentWidgets,
    ) {
        if self.close_preparation_pending() || self.relation_edits_pending(object) {
            return;
        }

        let mut renamed = object.clone();
        renamed.name = new_name.to_string();

        for tab in &mut self.browse_tabs {
            if tab.object == *object
                && !tab.pending_edits
                && !tab
                    .view
                    .as_ref()
                    .is_some_and(|view| view.model().has_pending_edits())
            {
                tab.object = renamed.clone();

                if let Some(view) = &tab.view {
                    view.emit(TableViewMsg::ObjectRenamed(renamed.clone()));
                }
                tab.page.set_title(new_name);
                tab.page
                    .set_tooltip(&format!("{}.{}", renamed.schema, renamed.name));
            }
        }

        self.sync_sidebar_selection(widgets);
    }

    pub(super) fn reload_browse_tab(&self, object: &DatabaseObject) {
        for tab in &self.browse_tabs {
            if tab.object == *object
                && let Some(view) = &tab.view
            {
                view.emit(TableViewMsg::ReloadContent);
            }
        }
    }

    pub(super) fn reload_browse_tab_after_structure_change(&self, target: &StructureActionTarget) {
        for tab in &self.browse_tabs {
            if tab.object == target.table
                && let Some(view) = &tab.view
            {
                view.emit(TableViewMsg::StructureChanged {
                    reset_browser_state: target.kind == StructureActionKind::Column,
                });
            }
        }
    }

    pub(super) fn remove_deleted_object(
        &mut self,
        object: &DatabaseObject,
        widgets: &WindowContentWidgets,
    ) {
        if self.relation_edits_pending(object) {
            self.table_navigation_blocked(widgets);

            return;
        }

        let pages = self
            .browse_tabs
            .iter()
            .filter(|tab| tab.object == *object)
            .map(|tab| tab.page.clone())
            .collect::<Vec<_>>();

        self.browse_tabs.retain(|tab| tab.object != *object);

        self.close_pages_immediately(widgets, pages);

        self.sync_sidebar_selection(widgets);
    }

    pub(super) fn sync_sidebar_selection(&self, widgets: &WindowContentWidgets) {
        if let Some(tab_id) = selected_browse_tab_id(widgets)
            && let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id)
        {
            self.sidebar.emit(ObjectSidebarMsg::SetSelectedObject(Some(
                tab.object.clone(),
            )));
        } else {
            self.sidebar.emit(ObjectSidebarMsg::SetSelectedObject(None));
        }
    }

    fn active_session_tab_id(&self, widgets: &WindowContentWidgets) -> Option<SavedSessionTabId> {
        let page = widgets.query_tab_view.selected_page()?;
        let child = page.child();

        if query_tab_id_from_widget(&child).is_some_and(|id| {
            self.query_tabs
                .iter()
                .any(|tab| tab.id == id && tab.document.is_some())
        }) {
            return None;
        }

        query_tab_id_from_widget(&child)
            .and_then(|id| self.query_tabs.iter().find(|tab| tab.id == id))
            .map(|tab| SavedSessionTabId::Query(tab.session_id))
            .or_else(|| browse_tab_id_from_widget(&child).map(SavedSessionTabId::Browse))
    }

    fn select_session_tab(
        &mut self,
        active_tab: Option<SavedSessionTabId>,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        match active_tab {
            Some(SavedSessionTabId::Query(tab_id)) => {
                if let Some(tab) = self.query_tabs.iter().find(|tab| tab.id == tab_id) {
                    self.active_query_tab_id = tab_id;
                    widgets.query_tab_view.set_selected_page(&tab.page);
                    return;
                }
            }
            Some(SavedSessionTabId::Browse(tab_id)) => {
                if let Some(tab) = self.browse_tabs.iter().find(|tab| tab.id == tab_id) {
                    widgets.query_tab_view.set_selected_page(&tab.page);
                    return;
                }
            }
            None => {}
        }

        self.select_active_query_tab(widgets, sender);
    }

    pub(super) fn select_active_query_tab(
        &mut self,
        widgets: &WindowContentWidgets,
        sender: &ComponentSender<Self>,
    ) {
        if self.active_query_tab().is_none() {
            self.add_query_tab(widgets, sender);
            return;
        }

        if let Some(tab) = self.active_query_tab() {
            widgets.query_tab_view.set_selected_page(&tab.page);
        }
    }

    pub(super) fn clear_browse_tabs(&mut self, widgets: &WindowContentWidgets) {
        let pages = self
            .browse_tabs
            .drain(..)
            .map(|tab| tab.page)
            .collect::<Vec<_>>();
        self.close_pages_immediately(widgets, pages);
    }

    fn clear_all_tabs(&mut self, widgets: &WindowContentWidgets) {
        let mut pages = Vec::with_capacity(self.tab_count());

        let mut retained = Vec::new();
        for mut tab in self.query_tabs.drain(..) {
            if tab.document.is_some() {
                retained.push(tab);
                continue;
            }
            if let Some(active_query) = tab.active_query.take() {
                active_query.abort_handle.abort();
            }

            pages.push(tab.page);
        }
        self.query_tabs = retained;

        for tab in self.browse_tabs.drain(..) {
            pages.push(tab.page);
        }

        self.close_pages_immediately(widgets, pages);
        self.active_query_tab_id = 0;
    }

    fn close_pages_immediately(
        &self,
        widgets: &WindowContentWidgets,
        pages: impl IntoIterator<Item = adw::TabPage>,
    ) {
        let tab_view_signals_blocked = self.tab_view_signals_blocked.clone();

        with_tab_view_signals_blocked(&tab_view_signals_blocked, || {
            for page in pages {
                widgets.query_tab_view.close_page(&page);
            }
        });
    }
}

fn with_tab_view_signals_blocked<R>(flag: &Rc<Cell<bool>>, f: impl FnOnce() -> R) -> R {
    let previous = flag.get();
    flag.set(true);
    let _guard = TabViewSignalBlockGuard { flag, previous };

    f()
}

struct TabViewSignalBlockGuard<'a> {
    flag: &'a Rc<Cell<bool>>,
    previous: bool,
}

impl Drop for TabViewSignalBlockGuard<'_> {
    fn drop(&mut self) {
        self.flag.set(self.previous);
    }
}

pub(super) fn query_tab_id_from_widget(widget: &gtk::Widget) -> Option<u64> {
    widget
        .widget_name()
        .strip_prefix("query-tab-")
        .and_then(|id| id.parse().ok())
}

pub(super) fn browse_tab_id_from_widget(widget: &gtk::Widget) -> Option<u64> {
    widget
        .widget_name()
        .strip_prefix("browse-tab-")
        .and_then(|id| id.parse().ok())
}

fn tab_id_from_widget_name(widget_name: Option<&str>, prefix: &str) -> Option<u64> {
    widget_name
        .and_then(|name| name.strip_prefix(prefix))
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

#[cfg(test)]
mod session_id_tests {
    use super::SessionTabIds;

    #[test]
    fn new_scratch_ids_never_reuse_promoted_sources_after_restart() {
        let mut ids = SessionTabIds::default();
        ids.reserve(17);
        ids.reserve(8);
        let first_window_tab = ids.allocate();
        let second_window_tab = ids.allocate();
        assert_eq!(first_window_tab, 18);
        assert_eq!(second_window_tab, 19);

        let mut restarted = SessionTabIds::default();
        restarted.reserve(17);
        restarted.reserve(first_window_tab);
        restarted.reserve(second_window_tab);
        assert_eq!(restarted.allocate(), 20);
    }

    #[test]
    fn re_restoring_a_saved_identity_does_not_reassign_it_or_reuse_it() {
        let mut ids = SessionTabIds::default();
        ids.reserve(42);
        let new_tab = ids.allocate();
        ids.reserve(42);
        assert_eq!(new_tab, 43);
        assert_eq!(ids.allocate(), 44);
    }

    #[test]
    fn maximum_legacy_id_does_not_wrap_into_a_reserved_source() {
        let mut ids = SessionTabIds::default();
        for id in [0, 1, u64::MAX] {
            ids.reserve(id);
        }
        assert_eq!(ids.allocate(), 2);
        assert_eq!(ids.allocate(), 3);
    }
}
