use futures_util::future::AbortHandle;
use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_browser::{
    DEFAULT_PAGE_SIZE, PAGE_SIZE_OPTIONS, TableCell, TableColumn, TableFilter, TablePage, TableSort,
};
use cell_editor::show_edit_cell_popover;
use filters::{FilterEvent, FilterPanel, initial_filter, validate_filter_values};
use grid::{TableBrowserRow, cell_factory, clear_columns};
use sorting::{connect_sort_handlers, next_sort_for_header_click, sync_sort_indicator};

mod cell_editor;
mod filters;
mod grid;
mod loading;
mod sorting;

pub struct TableBrowser {
    pool: Option<PgPool>,
    object: Option<DatabaseObject>,
    page: Option<TablePage>,
    is_loading: bool,
    is_error: bool,
    status_title: String,
    status_description: Option<String>,
    offset: u32,
    page_size: u32,
    available_columns: Vec<TableColumn>,
    draft_filters: Vec<TableFilter>,
    active_filters: Vec<TableFilter>,
    sort: Option<TableSort>,
    filters_expanded: bool,
    page_generation: u64,
    request_id: u64,
    active_request_id: Option<u64>,
    active_last_page_request_id: Option<u64>,
    active_abort_handle: Option<AbortHandle>,
    table_rows: gio::ListStore,
    table_view: gtk::ColumnView,
    filter_panel: gtk::Box,
    edit_popover: Option<gtk::Popover>,
    rendered_columns: Vec<String>,
    style_manager: adw::StyleManager,
    dark_notify_handler: Option<glib::SignalHandlerId>,
}

#[derive(Debug)]
pub enum TableBrowserMsg {
    Open {
        pool: PgPool,
        object: DatabaseObject,
    },
    ObjectRenamed(DatabaseObject),
    Refresh,
    FirstPage,
    PreviousPage,
    NextPage,
    LastPage,
    PageSizeChanged(u32),
    ToggleFilters,
    FilterEvent(FilterEvent),
    SortChanged(TableSort),
    PageLoaded {
        id: u64,
        result: Result<TablePage, String>,
    },
    LastPageOffsetLoaded {
        id: u64,
        result: Result<u32, String>,
    },
    EditCellRequested {
        anchor: gtk::Label,
        row_index: usize,
        column_index: usize,
    },
    CellEditSubmitted {
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        value: Option<String>,
    },
    CellUpdated {
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        result: Result<TableCell, String>,
    },
    AppearanceChanged,
}

#[derive(Debug)]
pub enum TableBrowserCommandOutput {
    PageLoaded {
        id: u64,
        result: Result<TablePage, String>,
    },
    LastPageOffsetLoaded {
        id: u64,
        result: Result<u32, String>,
    },
    CellUpdated {
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        result: Result<TableCell, String>,
    },
}

#[relm4::component(pub)]
impl Component for TableBrowser {
    type Init = ();
    type Input = TableBrowserMsg;
    type Output = ();
    type CommandOutput = TableBrowserCommandOutput;

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 0,
            add_css_class: "table-browser",

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 8,
                set_margin_top: 8,
                set_margin_bottom: 8,
                set_margin_start: 12,
                set_margin_end: 12,
                add_css_class: "table-browser-header",
                #[watch]
                set_visible: model.object.is_some(),

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 0,
                    set_hexpand: true,

                    gtk::Label {
                        add_css_class: "heading",
                        set_halign: gtk::Align::Start,
                        #[watch]
                        set_label: &model.object_title(),
                    },

                    gtk::Label {
                        add_css_class: "caption",
                        add_css_class: "dim-label",
                        set_halign: gtk::Align::Start,
                        #[watch]
                        set_label: &model.object_context(),
                    },
                },

                gtk::Button {
                    #[watch]
                    set_child: Some(&icon_label_widget("filter-symbolic", &model.filter_button_label())),
                    set_tooltip_text: Some(&gettext("Show or edit filters")),
                    add_css_class: "flat",
                    #[watch]
                    set_visible: model.object.is_some(),
                    connect_clicked => TableBrowserMsg::ToggleFilters,
                },

                gtk::Button {
                    set_icon_name: "view-refresh-symbolic",
                    set_tooltip_text: Some(&gettext("Refresh")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.object.is_some() && !model.is_loading,
                    connect_clicked => TableBrowserMsg::Refresh,
                },
            },

            gtk::Revealer {
                set_transition_type: gtk::RevealerTransitionType::SlideDown,
                #[watch]
                set_reveal_child: model.filters_expanded && model.object.is_some(),

                #[wrap(Some)]
                set_child = &model.filter_panel.clone(),
            },

            #[name = "stack"]
            gtk::Stack {
                set_vexpand: true,

                add_named[Some("status")] = &adw::StatusPage {
                    #[watch]
                    set_icon_name: Some(model.status_icon_name()),
                    #[watch]
                    set_title: &model.status_title,
                    #[watch]
                    set_description: model.status_description.as_deref(),

                    #[wrap(Some)]
                    set_child = &gtk::Spinner {
                        #[watch]
                        set_visible: model.is_loading,
                        #[watch]
                        set_spinning: model.is_loading,
                    },
                },

                #[name = "grid"]
                add_named[Some("grid")] = &gtk::ScrolledWindow {
                    set_hexpand: true,
                    set_vexpand: true,
                    set_policy: (gtk::PolicyType::Automatic, gtk::PolicyType::Automatic),
                    add_css_class: "table-browser-scroller",
                },
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 8,
                set_margin_top: 6,
                set_margin_bottom: 8,
                set_margin_start: 12,
                set_margin_end: 12,
                add_css_class: "table-browser-footer",
                #[watch]
                set_visible: model.object.is_some(),

                gtk::Button {
                    set_icon_name: "go-first-symbolic",
                    set_tooltip_text: Some(&gettext("First page")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.can_go_previous(),
                    connect_clicked => TableBrowserMsg::FirstPage,
                },

                gtk::Button {
                    set_icon_name: "go-previous-symbolic",
                    set_tooltip_text: Some(&gettext("Previous page")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.can_go_previous(),
                    connect_clicked => TableBrowserMsg::PreviousPage,
                },

                gtk::Box {
                    set_hexpand: true,
                },

                gtk::Spinner {
                    #[watch]
                    set_visible: model.is_loading,
                    #[watch]
                    set_spinning: model.is_loading,
                },

                gtk::Label {
                    add_css_class: "caption",
                    add_css_class: "dim-label",
                    #[watch]
                    set_label: &model.footer_text(),
                },

                gtk::Box {
                    set_hexpand: true,
                },

                gtk::Button {
                    set_icon_name: "go-next-symbolic",
                    set_tooltip_text: Some(&gettext("Next page")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.can_go_next(),
                    connect_clicked => TableBrowserMsg::NextPage,
                },

                gtk::Button {
                    set_icon_name: "go-last-symbolic",
                    set_tooltip_text: Some(&gettext("Last page")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.can_go_next(),
                    connect_clicked => TableBrowserMsg::LastPage,
                },

                gtk::Label {
                    add_css_class: "caption",
                    add_css_class: "dim-label",
                    set_label: &gettext("Rows"),
                },

                gtk::DropDown {
                    add_css_class: "compact",
                    set_tooltip_text: Some(&gettext("Rows per page")),
                    set_model: Some(&page_size_model()),
                    #[watch]
                    set_selected: model.selected_page_size_index(),
                    connect_selected_notify[sender] => move |dropdown| {
                        let selected = dropdown.selected() as usize;

                        if let Some(page_size) = PAGE_SIZE_OPTIONS.get(selected).copied() {
                            sender.input(TableBrowserMsg::PageSizeChanged(page_size));
                        }
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
        let table_rows = gio::ListStore::new::<glib::BoxedAnyObject>();
        let filter_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let table_view = gtk::ColumnView::new(Some(gtk::NoSelection::new(Some(
            table_rows.clone().upcast::<gio::ListModel>(),
        ))));
        table_view.set_vexpand(true);
        table_view.set_hexpand(true);
        table_view.set_show_row_separators(true);
        table_view.set_show_column_separators(true);
        table_view.add_css_class("data-table");
        let style_manager = adw::StyleManager::default();
        let dark_notify_handler = {
            let sender = sender.clone();
            style_manager.connect_dark_notify(move |_| {
                sender.input(TableBrowserMsg::AppearanceChanged);
            })
        };

        let model = TableBrowser {
            pool: None,
            object: None,
            page: None,
            is_loading: false,
            is_error: false,
            status_title: gettext("Select a table"),
            status_description: Some(gettext(
                "Choose a table or view from the sidebar to browse its rows.",
            )),
            offset: 0,
            page_size: DEFAULT_PAGE_SIZE,
            available_columns: Vec::new(),
            draft_filters: Vec::new(),
            active_filters: Vec::new(),
            sort: None,
            filters_expanded: false,
            page_generation: 0,
            request_id: 0,
            active_request_id: None,
            active_last_page_request_id: None,
            active_abort_handle: None,
            table_rows,
            table_view,
            filter_panel,
            edit_popover: None,
            rendered_columns: Vec::new(),
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
        };

        let widgets = view_output!();
        widgets.grid.set_child(Some(&model.table_view));
        connect_sort_handlers(&model.table_view, &sender);
        FilterPanel::rebuild(
            &model.filter_panel,
            model.filter_columns(),
            &model.draft_filters,
            !model.active_filters.is_empty(),
            &sender,
        );
        set_stack_child(&widgets, false);
        root.set_spacing(0);

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
            TableBrowserMsg::Open { pool, object } => {
                self.pool = Some(pool);
                self.object = Some(object);
                self.offset = 0;
                self.available_columns.clear();
                self.draft_filters.clear();
                self.active_filters.clear();
                self.sort = None;
                self.filters_expanded = false;
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::ObjectRenamed(object) => {
                self.object = Some(object);
                self.offset = 0;
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::Refresh => {
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::FirstPage => {
                if self.can_go_previous() {
                    self.offset = 0;
                    self.load_page(widgets, &sender);
                }
            }

            TableBrowserMsg::PreviousPage => {
                self.offset = self.offset.saturating_sub(self.page_size);
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::NextPage => {
                if self.can_go_next() {
                    self.offset = self.offset.saturating_add(self.page_size);
                    self.load_page(widgets, &sender);
                }
            }

            TableBrowserMsg::LastPage => {
                if self.can_go_next() {
                    self.load_last_page_offset(&sender);
                }
            }

            TableBrowserMsg::PageSizeChanged(page_size) => {
                if self.page_size == page_size {
                    return;
                }

                self.page_size = page_size;
                self.offset = 0;
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::ToggleFilters => {
                self.filters_expanded = !self.filters_expanded;

                if self.filters_expanded
                    && self.draft_filters.is_empty()
                    && self.active_filters.is_empty()
                    && let Some(filter) = initial_filter(&self.available_columns)
                {
                    self.draft_filters.push(filter);
                    self.rebuild_filters(widgets, &sender);
                }
            }

            TableBrowserMsg::FilterEvent(event) => {
                self.handle_filter_event(event, widgets, &sender, root);
            }

            TableBrowserMsg::SortChanged(sort) => {
                let next_sort = next_sort_for_header_click(self.sort.as_ref(), sort);

                if self.sort == next_sort {
                    return;
                }

                self.sort = next_sort;
                self.offset = 0;
                sync_sort_indicator(&self.table_view, self.sort.as_ref());
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::PageLoaded { id, result } => {
                if self.active_request_id != Some(id) {
                    return;
                }

                self.active_request_id = None;
                self.active_abort_handle = None;
                self.is_loading = false;

                match result {
                    Ok(page) => {
                        self.is_error = false;
                        self.status_title.clear();
                        self.status_description = None;
                        self.available_columns.clone_from(&page.columns);
                        if self.sort.as_ref().is_some_and(|sort| {
                            !page
                                .columns
                                .iter()
                                .any(|column| column.name == sort.column_name)
                        }) {
                            self.sort = None;
                            sync_sort_indicator(&self.table_view, self.sort.as_ref());
                        }
                        self.page = Some(page);
                        self.page_generation = self.page_generation.wrapping_add(1);
                    }

                    Err(error) => {
                        self.is_error = true;
                        self.status_title = gettext("Loading table failed");
                        self.status_description = Some(error);
                        self.page = None;
                    }
                }

                self.render_table(&sender);
                self.rebuild_filters(widgets, &sender);
                set_stack_child(widgets, self.page.is_some());
            }

            TableBrowserMsg::LastPageOffsetLoaded { id, result } => {
                if self.active_last_page_request_id != Some(id) {
                    return;
                }

                self.active_last_page_request_id = None;
                self.active_abort_handle = None;
                self.is_loading = false;

                match result {
                    Ok(offset) => {
                        self.offset = offset;
                        self.load_page(widgets, &sender);
                    }

                    Err(error) => {
                        self.show_warning(root, &gettext("Loading last page failed"), &error);
                    }
                }
            }

            TableBrowserMsg::EditCellRequested {
                anchor,
                row_index,
                column_index,
            } => {
                self.show_edit_popover(&anchor, row_index, column_index, &sender, root);
            }

            TableBrowserMsg::CellEditSubmitted {
                page_generation,
                row_index,
                column_index,
                value,
            } => {
                self.update_cell(page_generation, row_index, column_index, value, &sender);
            }

            TableBrowserMsg::CellUpdated {
                page_generation,
                row_index,
                column_index,
                result,
            } => {
                if let Err(error) =
                    self.handle_cell_updated(page_generation, row_index, column_index, result)
                {
                    self.show_warning(root, &gettext("Saving cell failed"), &error);
                }

                self.render_table(&sender);
            }

            TableBrowserMsg::AppearanceChanged => {
                close_popover(&mut self.edit_popover);
                self.rendered_columns.clear();
                self.render_table(&sender);
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
        sender.input(match msg {
            TableBrowserCommandOutput::PageLoaded { id, result } => {
                TableBrowserMsg::PageLoaded { id, result }
            }
            TableBrowserCommandOutput::LastPageOffsetLoaded { id, result } => {
                TableBrowserMsg::LastPageOffsetLoaded { id, result }
            }
            TableBrowserCommandOutput::CellUpdated {
                page_generation,
                row_index,
                column_index,
                result,
            } => TableBrowserMsg::CellUpdated {
                page_generation,
                row_index,
                column_index,
                result,
            },
        });
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        if let Some(handler) = self.dark_notify_handler.take() {
            self.style_manager.disconnect(handler);
        }

        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }

        close_popover(&mut self.edit_popover);
    }
}

impl TableBrowser {
    fn render_table(&mut self, sender: &ComponentSender<Self>) {
        let Some(page) = self.page.clone() else {
            self.table_rows.remove_all();
            return;
        };

        self.sync_columns(&page.columns, sender);
        self.table_rows.remove_all();

        for (index, row) in page.rows.iter().enumerate() {
            self.table_rows
                .append(&glib::BoxedAnyObject::new(TableBrowserRow {
                    index,
                    cells: row.clone(),
                }));
        }
    }

    fn rebuild_filters(&self, _widgets: &TableBrowserWidgets, sender: &ComponentSender<Self>) {
        FilterPanel::rebuild(
            &self.filter_panel,
            self.filter_columns(),
            &self.draft_filters,
            !self.active_filters.is_empty(),
            sender,
        );
    }

    fn filter_columns(&self) -> Option<&[TableColumn]> {
        if self.available_columns.is_empty() {
            None
        } else {
            Some(&self.available_columns)
        }
    }

    fn handle_filter_event(
        &mut self,
        event: FilterEvent,
        widgets: &TableBrowserWidgets,
        sender: &ComponentSender<Self>,
        root: &gtk::Box,
    ) {
        match event {
            FilterEvent::DraftChanged(filters) => {
                self.draft_filters = filters;
                self.rebuild_filters(widgets, sender);
            }
            FilterEvent::DraftValuesChanged(filters) => {
                self.draft_filters = filters;
            }
            FilterEvent::Apply(filters) => {
                self.apply_filters(filters, widgets, sender, root);
            }
            FilterEvent::Clear => {
                self.draft_filters.clear();
                self.active_filters.clear();
                self.rebuild_filters(widgets, sender);
                self.offset = 0;
                self.load_page(widgets, sender);
            }
        }
    }

    fn apply_filters(
        &mut self,
        filters: Vec<TableFilter>,
        widgets: &TableBrowserWidgets,
        sender: &ComponentSender<Self>,
        root: &gtk::Box,
    ) {
        if let Err(error) = validate_filter_values(&filters) {
            self.show_warning(root, &gettext("Filter cannot be applied"), &error);
            return;
        }

        self.draft_filters.clone_from(&filters);
        self.active_filters = filters;
        self.filters_expanded = !self.active_filters.is_empty();
        self.offset = 0;
        self.load_page(widgets, sender);
    }

    fn sync_columns(&mut self, columns: &[TableColumn], sender: &ComponentSender<Self>) {
        let column_keys = columns
            .iter()
            .map(|column| format!("{}\u{1f}{}", column.name, column.display_type))
            .collect::<Vec<_>>();

        if self.rendered_columns == column_keys {
            return;
        }

        clear_columns(&self.table_view);
        self.rendered_columns = column_keys;

        let is_dark = self.style_manager.is_dark();

        for (index, column) in columns.iter().enumerate() {
            let factory = cell_factory(index, column.type_group, is_dark, sender);
            let title = column.name.clone();
            let view_column = gtk::ColumnViewColumn::new(Some(&title), Some(factory));
            view_column.set_resizable(true);
            view_column.set_expand(index < 3);
            view_column.set_sorter(Some(&gtk::CustomSorter::new(|_, _| {
                std::cmp::Ordering::Equal.into()
            })));
            self.table_view.append_column(&view_column);
        }

        sync_sort_indicator(&self.table_view, self.sort.as_ref());
    }

    fn object_title(&self) -> String {
        self.object
            .as_ref()
            .map(|object| object.name.clone())
            .unwrap_or_else(|| gettext("Table Browser"))
    }

    fn object_context(&self) -> String {
        let Some(object) = self.object.as_ref() else {
            return String::new();
        };

        let kind = match object.kind {
            DatabaseObjectKind::Table => gettext("Table"),
            DatabaseObjectKind::View => gettext("View"),
        };

        format!("{} · {}", object.schema, kind)
    }

    fn filter_button_label(&self) -> String {
        if self.active_filters.is_empty() {
            gettext("Filters")
        } else {
            format!("{} ({})", gettext("Filters"), self.active_filters.len())
        }
    }

    fn footer_text(&self) -> String {
        if self.is_loading {
            return gettext("Loading...");
        }

        let Some(page) = self.page.as_ref() else {
            return String::new();
        };

        if page.rows.is_empty() {
            return gettext("No rows");
        }

        let first = page.offset + 1;
        let last = page.offset + u32::try_from(page.rows.len()).unwrap_or(u32::MAX);
        let row_count = u32::try_from(page.rows.len()).unwrap_or(u32::MAX);

        format!("{first}-{last} {}", ngettext("row", "rows", row_count))
    }

    fn selected_page_size_index(&self) -> u32 {
        PAGE_SIZE_OPTIONS
            .iter()
            .position(|option| *option == self.page_size)
            .unwrap_or(1) as u32
    }

    fn can_go_previous(&self) -> bool {
        !self.is_loading && self.offset > 0
    }

    fn can_go_next(&self) -> bool {
        !self.is_loading && self.page.as_ref().is_some_and(|page| page.has_next_page)
    }

    fn status_icon_name(&self) -> &'static str {
        if self.is_error {
            "dialog-error-symbolic"
        } else if self.is_loading {
            "view-refresh-symbolic"
        } else {
            "table"
        }
    }

    fn show_edit_popover(
        &mut self,
        anchor: &gtk::Label,
        row_index: usize,
        column_index: usize,
        sender: &ComponentSender<Self>,
        root: &gtk::Box,
    ) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let Some(column) = page.columns.get(column_index) else {
            return;
        };
        let Some(cell) = page
            .rows
            .get(row_index)
            .and_then(|row| row.get(column_index))
        else {
            return;
        };

        if let Err(error) = validate_editable_cell(&page.object, &page.columns, column_index) {
            self.show_warning(root, &gettext("Cell cannot be edited"), &error);
            return;
        }

        close_popover(&mut self.edit_popover);

        self.edit_popover = Some(show_edit_cell_popover(
            anchor,
            column,
            cell,
            sender.clone(),
            self.page_generation,
            row_index,
            column_index,
        ));
    }

    fn update_cell(
        &mut self,
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        value: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        if page_generation != self.page_generation {
            return;
        }

        close_popover(&mut self.edit_popover);

        let (Some(pool), Some(page)) = (self.pool.clone(), self.page.clone()) else {
            return;
        };
        let Some(row) = page.rows.get(row_index).cloned() else {
            return;
        };

        sender.oneshot_command(async move {
            let result = db::browser::update_table_cell(
                &pool,
                &page.object,
                &page.columns,
                &row,
                column_index,
                value,
            )
            .await
            .map_err(|error| error.to_string());

            TableBrowserCommandOutput::CellUpdated {
                page_generation,
                row_index,
                column_index,
                result,
            }
        });
    }

    fn handle_cell_updated(
        &mut self,
        page_generation: u64,
        row_index: usize,
        column_index: usize,
        result: Result<TableCell, String>,
    ) -> Result<(), String> {
        if page_generation != self.page_generation {
            return Ok(());
        }

        let Some(page) = self.page.as_mut() else {
            return Ok(());
        };

        match result {
            Ok(cell) => {
                if let Some(row) = page.rows.get_mut(row_index)
                    && let Some(value) = row.get_mut(column_index)
                {
                    *value = cell;
                }

                Ok(())
            }

            Err(error) => Err(error),
        }
    }

    fn show_warning(&self, root: &gtk::Box, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .close_response("close")
            .build();

        dialog.add_response("close", &gettext("Close"));
        dialog.present(root.root().and_downcast::<gtk::Window>().as_ref());
    }
}

fn validate_editable_cell(
    object: &DatabaseObject,
    columns: &[TableColumn],
    column_index: usize,
) -> Result<(), String> {
    if object.kind != DatabaseObjectKind::Table {
        return Err(gettext("Only tables can be edited."));
    }

    let Some(column) = columns.get(column_index) else {
        return Err(gettext("The selected cell is no longer available."));
    };

    if column.is_primary_key {
        return Err(gettext("Primary key columns are read-only for now."));
    }

    if !column.is_editable_value_type() {
        return Err(gettext("This column type is not editable yet."));
    }

    if !columns.iter().any(|column| column.is_primary_key) {
        return Err(gettext("Editing requires a primary key."));
    }

    Ok(())
}

fn close_popover(popover: &mut Option<gtk::Popover>) {
    if let Some(popover) = popover.take() {
        popover.popdown();

        if popover.parent().is_some() {
            popover.unparent();
        }
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

fn set_stack_child(widgets: &TableBrowserWidgets, has_page: bool) {
    widgets
        .stack
        .set_visible_child_name(if has_page { "grid" } else { "status" });
}

fn page_size_model() -> gtk::StringList {
    let labels = PAGE_SIZE_OPTIONS
        .iter()
        .map(|page_size| page_size.to_string())
        .collect::<Vec<_>>();
    let borrowed = labels.iter().map(String::as_str).collect::<Vec<_>>();

    gtk::StringList::new(&borrowed)
}

#[cfg(test)]
mod tests {
    use super::loading::last_page_offset;

    #[test]
    fn last_page_offset_handles_empty_pages() {
        assert_eq!(last_page_offset(0, 100), 0);
        assert_eq!(last_page_offset(-1, 100), 0);
    }

    #[test]
    fn last_page_offset_points_to_first_row_of_last_page() {
        assert_eq!(last_page_offset(1, 100), 0);
        assert_eq!(last_page_offset(100, 100), 0);
        assert_eq!(last_page_offset(101, 100), 100);
        assert_eq!(last_page_offset(250, 100), 200);
    }
}
