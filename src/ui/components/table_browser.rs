use futures_util::future::AbortHandle;
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;
use sqlx::PgPool;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::result_copy;
use crate::models::table_browser::{
    DEFAULT_PAGE_SIZE, PAGE_SIZE_OPTIONS, TableCell, TableColumn, TableFilter, TableInsertValue,
    TablePage, TableSort,
};
use crate::models::{csv_export, csv_export::CsvExportOptions};
use crate::ui::components::csv_export_dialog::{
    show_csv_export_options_dialog, show_csv_save_dialog,
};
use filters::{FilterEvent, FilterPanel, initial_filter, validate_filter_values};
use grid::render_table;
use sorting::{connect_sort_handlers, next_sort_for_header_click, sync_sort_indicator};

mod cell_editor;
mod delete_row;
mod editing;
mod filters;
mod grid;
mod insert_row;
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
    show_header: bool,
    page_generation: u64,
    request_id: u64,
    active_request_id: Option<u64>,
    active_last_page_request_id: Option<u64>,
    active_insert_request_id: Option<u64>,
    active_delete_request_id: Option<u64>,
    active_abort_handle: Option<AbortHandle>,
    table_rows: gio::ListStore,
    selection: gtk::SingleSelection,
    table_view: gtk::ColumnView,
    filter_panel: gtk::Box,
    edit_popover: Option<gtk::Popover>,
    rendered_columns: Vec<String>,
    copy_target: Rc<Cell<Option<CopyTarget>>>,
    edit_target: Rc<RefCell<Option<EditTarget>>>,
    edit_action: gio::SimpleAction,
    delete_action: gio::SimpleAction,
    context_popover: gtk::PopoverMenu,
    style_manager: adw::StyleManager,
    dark_notify_handler: Option<glib::SignalHandlerId>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CopyTarget {
    pub(super) row_index: usize,
    pub(super) column_index: usize,
}

#[derive(Debug, Clone)]
pub(super) struct EditTarget {
    pub(super) anchor: gtk::Label,
    pub(super) row_index: usize,
    pub(super) column_index: usize,
}

impl CopyTarget {
    fn cell_message(self) -> TableBrowserMsg {
        TableBrowserMsg::CopyCell {
            row_index: self.row_index,
            column_index: self.column_index,
        }
    }

    fn row_message(self) -> TableBrowserMsg {
        TableBrowserMsg::CopyRow(self.row_index)
    }

    fn column_message(self) -> TableBrowserMsg {
        TableBrowserMsg::CopyColumn(self.column_index)
    }
}

#[derive(Debug)]
pub enum TableBrowserMsg {
    Open {
        pool: PgPool,
        object: DatabaseObject,
    },
    ObjectRenamed(DatabaseObject),
    Refresh,
    SchemaChanged,
    SetHeaderVisible(bool),
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
    CopyCell {
        row_index: usize,
        column_index: usize,
    },
    CopyRow(usize),
    CopyColumn(usize),
    CopyPage,
    ExportCsvRequested,
    ExportCsvConfirmed {
        options: CsvExportOptions,
        path: PathBuf,
    },
    CsvExported(Result<(), String>),
    InsertRowRequested,
    InsertRowConfirmed(Vec<TableInsertValue>),
    RowInserted {
        id: u64,
        result: InsertRowResult,
    },
    DeleteSelectedRowRequested,
    DeleteRowConfirmed {
        page_generation: u64,
        row_index: usize,
    },
    RowDeleted {
        id: u64,
        result: DeleteRowResult,
    },
    SelectionChanged,
    EditCellFromMenu {
        anchor: gtk::Label,
        row_index: usize,
        column_index: usize,
    },
    AppearanceChanged,
}

#[derive(Debug)]
pub enum TableBrowserOutput {
    Copied(String),
    Exported(String),
    Inserted(String),
    Deleted(String),
    SelectionChanged(bool),
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
    CsvExported(Result<(), String>),
    RowInserted {
        id: u64,
        result: InsertRowResult,
    },
    RowDeleted {
        id: u64,
        result: DeleteRowResult,
    },
}

#[derive(Debug)]
pub enum InsertRowResult {
    Inserted(TablePage),
    InsertFailed(String),
    ReloadFailed(String),
}

#[derive(Debug)]
pub enum DeleteRowResult {
    Deleted(TablePage),
    DeleteFailed(String),
    ReloadFailed(String),
}

#[relm4::component(pub)]
impl Component for TableBrowser {
    type Init = ();
    type Input = TableBrowserMsg;
    type Output = TableBrowserOutput;
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
                set_visible: model.show_header && model.object.is_some(),

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
                    set_tooltip_text: Some(&gettext("Export CSV")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.object.is_some() && !model.is_loading,
                    set_child: Some(&icon_label_widget("document-save-symbolic", &gettext("Export"))),
                    connect_clicked => TableBrowserMsg::ExportCsvRequested,
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
                    set_icon_name: "first-symbolic",
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
                    set_icon_name: "last-symbolic",
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
        let selection = gtk::SingleSelection::new(Some(table_rows.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        selection.connect_selected_notify({
            let sender = sender.clone();

            move |_| sender.input(TableBrowserMsg::SelectionChanged)
        });

        let table_view = gtk::ColumnView::new(Some(selection.clone()));

        table_view.set_vexpand(true);
        table_view.set_hexpand(true);
        table_view.set_show_row_separators(true);
        table_view.set_show_column_separators(true);
        table_view.set_focusable(true);
        table_view.add_css_class("data-table");
        table_view.add_controller(delete_key_controller(&sender));

        let copy_target = Rc::new(Cell::new(None));
        let edit_target = Rc::new(RefCell::new(None));

        let context_popover = gtk::PopoverMenu::from_model(Some(&context_menu()));
        context_popover.set_has_arrow(false);
        context_popover.set_parent(&root);

        let (context_action_group, edit_action, delete_action) =
            context_action_group(copy_target.clone(), edit_target.clone(), sender.clone());

        root.insert_action_group("browser", Some(&context_action_group));

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
            show_header: true,
            page_generation: 0,
            request_id: 0,
            active_request_id: None,
            active_last_page_request_id: None,
            active_insert_request_id: None,
            active_delete_request_id: None,
            active_abort_handle: None,
            table_rows,
            selection,
            table_view,
            filter_panel,
            edit_popover: None,
            rendered_columns: Vec::new(),
            copy_target,
            edit_target,
            edit_action,
            delete_action,
            context_popover,
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
                self.close_context_menu();
                self.clear_selection();
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
                self.close_context_menu();
                self.clear_selection();
                self.object = Some(object);
                self.offset = 0;
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::Refresh => {
                self.close_context_menu();
                self.clear_selection();
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::SchemaChanged => {
                self.close_context_menu();
                self.clear_selection();
                close_popover(&mut self.edit_popover);
                self.offset = 0;
                self.available_columns.clear();
                self.draft_filters.clear();
                self.active_filters.clear();
                self.sort = None;
                self.filters_expanded = false;
                sync_sort_indicator(&self.table_view, self.sort.as_ref());
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::SetHeaderVisible(visible) => {
                self.show_header = visible;
            }

            TableBrowserMsg::FirstPage => {
                if self.can_go_previous() {
                    self.clear_selection();
                    self.offset = 0;
                    self.load_page(widgets, &sender);
                }
            }

            TableBrowserMsg::PreviousPage => {
                self.clear_selection();
                self.offset = self.offset.saturating_sub(self.page_size);
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::NextPage => {
                if self.can_go_next() {
                    self.clear_selection();
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

                self.close_context_menu();
                self.clear_selection();
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
                self.close_context_menu();
                self.clear_selection();
                self.offset = 0;
                sync_sort_indicator(&self.table_view, self.sort.as_ref());
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::PageLoaded { id, result } => {
                if self.active_request_id != Some(id) {
                    return;
                }

                self.close_context_menu();
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
                        self.clear_selection();
                    }

                    Err(error) => {
                        self.is_error = true;
                        self.status_title = gettext("Loading table failed");
                        self.status_description = Some(error);
                        self.page = None;
                        self.clear_selection();
                    }
                }

                render_table(self, &sender);
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
                if self.is_loading {
                    return;
                }

                self.open_edit_popover(&anchor, row_index, column_index, &sender, root);
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

                render_table(self, &sender);
            }

            TableBrowserMsg::CopyCell {
                row_index,
                column_index,
            } => {
                self.copy_text(
                    self.page
                        .as_ref()
                        .and_then(|page| result_copy::page_cell(page, row_index, column_index)),
                    gettext("Cell copied."),
                    &sender,
                );
                return;
            }

            TableBrowserMsg::CopyRow(row_index) => {
                self.copy_text(
                    self.page
                        .as_ref()
                        .and_then(|page| result_copy::page_row(page, row_index)),
                    gettext("Row copied."),
                    &sender,
                );
                return;
            }

            TableBrowserMsg::CopyColumn(column_index) => {
                self.copy_text(
                    self.page
                        .as_ref()
                        .and_then(|page| result_copy::page_column(page, column_index)),
                    gettext("Column copied."),
                    &sender,
                );
                return;
            }

            TableBrowserMsg::CopyPage => {
                self.copy_text(
                    self.page.as_ref().map(result_copy::page),
                    gettext("Page copied."),
                    &sender,
                );
                return;
            }

            TableBrowserMsg::ExportCsvRequested => {
                self.open_export_dialog(root, &sender);
                return;
            }

            TableBrowserMsg::ExportCsvConfirmed { options, path } => {
                self.export_csv(options, path, &sender);
                return;
            }

            TableBrowserMsg::CsvExported(result) => {
                match result {
                    Ok(()) => {
                        let _ =
                            sender.output(TableBrowserOutput::Exported(gettext("CSV exported.")));
                    }
                    Err(error) => {
                        self.show_warning(root, &gettext("Export failed"), &error);
                    }
                }

                return;
            }

            TableBrowserMsg::InsertRowRequested => {
                self.open_insert_row_dialog(root, &sender);
                return;
            }

            TableBrowserMsg::InsertRowConfirmed(values) => {
                self.clear_selection();
                self.insert_row(values, &sender);
            }

            TableBrowserMsg::RowInserted { id, result } => {
                if self.active_insert_request_id != Some(id) {
                    return;
                }

                self.active_insert_request_id = None;
                self.is_loading = false;

                match result {
                    InsertRowResult::Inserted(page) => {
                        let _ =
                            sender.output(TableBrowserOutput::Inserted(gettext("Row inserted.")));
                        self.is_error = false;
                        self.status_title.clear();
                        self.status_description = None;
                        self.available_columns.clone_from(&page.columns);
                        self.page = Some(page);
                        self.page_generation = self.page_generation.wrapping_add(1);
                        self.clear_selection();
                        render_table(self, &sender);
                        self.rebuild_filters(widgets, &sender);
                        set_stack_child(widgets, true);
                    }
                    InsertRowResult::InsertFailed(error) => {
                        self.show_warning(root, &gettext("Inserting row failed"), &error);
                    }
                    InsertRowResult::ReloadFailed(error) => {
                        self.show_warning(root, &gettext("Reloading table failed"), &error)
                    }
                }
            }

            TableBrowserMsg::DeleteSelectedRowRequested => {
                self.open_delete_row_dialog(root, &sender);
                return;
            }

            TableBrowserMsg::DeleteRowConfirmed {
                page_generation,
                row_index,
            } => {
                self.delete_row(page_generation, row_index, &sender);
            }

            TableBrowserMsg::RowDeleted { id, result } => {
                if self.active_delete_request_id != Some(id) {
                    return;
                }

                self.active_delete_request_id = None;
                self.is_loading = false;

                match result {
                    DeleteRowResult::Deleted(page) => {
                        let _ = sender.output(TableBrowserOutput::Deleted(gettext("Row deleted.")));
                        self.is_error = false;
                        self.status_title.clear();
                        self.status_description = None;
                        self.offset = page.offset;
                        self.available_columns.clone_from(&page.columns);
                        self.page = Some(page);
                        self.page_generation = self.page_generation.wrapping_add(1);
                        self.clear_selection();
                        render_table(self, &sender);
                        self.rebuild_filters(widgets, &sender);
                        set_stack_child(widgets, true);
                    }
                    DeleteRowResult::DeleteFailed(error) => {
                        self.show_warning(root, &gettext("Deleting row failed"), &error);
                    }
                    DeleteRowResult::ReloadFailed(error) => {
                        self.show_warning(root, &gettext("Reloading table failed"), &error)
                    }
                }
            }

            TableBrowserMsg::SelectionChanged => {
                let _ = sender.output(TableBrowserOutput::SelectionChanged(
                    self.can_delete_selected_row(),
                ));
                return;
            }

            TableBrowserMsg::EditCellFromMenu {
                anchor,
                row_index,
                column_index,
            } => {
                if self.is_loading {
                    return;
                }

                self.open_edit_popover(&anchor, row_index, column_index, &sender, root);
            }

            TableBrowserMsg::AppearanceChanged => {
                self.close_context_menu();
                close_popover(&mut self.edit_popover);
                self.rendered_columns.clear();
                render_table(self, &sender);
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
            TableBrowserCommandOutput::CsvExported(result) => TableBrowserMsg::CsvExported(result),
            TableBrowserCommandOutput::RowInserted { id, result } => {
                TableBrowserMsg::RowInserted { id, result }
            }
            TableBrowserCommandOutput::RowDeleted { id, result } => {
                TableBrowserMsg::RowDeleted { id, result }
            }
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
        self.close_context_menu();

        if self.context_popover.parent().is_some() {
            self.context_popover.unparent();
        }
    }
}

impl TableBrowser {
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

    fn copy_text(
        &self,
        text: Option<String>,
        message: String,
        sender: &ComponentSender<TableBrowser>,
    ) {
        let Some(text) = text else {
            return;
        };

        copy_text_to_clipboard(&text);
        let _ = sender.output(TableBrowserOutput::Copied(message));
    }

    fn close_context_menu(&self) {
        self.context_popover.popdown();
        self.copy_target.set(None);
        self.edit_target.borrow_mut().take();
        self.edit_action.set_enabled(false);
        self.delete_action.set_enabled(false);
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

        gettext("{first}-{last} rows")
            .replace("{first}", &first.to_string())
            .replace("{last}", &last.to_string())
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

    fn selected_row_index(&self) -> Option<usize> {
        let position = self.selection.selected();
        if position == gtk::INVALID_LIST_POSITION {
            return None;
        }

        usize::try_from(position).ok()
    }

    fn selected_row(&self) -> Option<(usize, Vec<TableCell>)> {
        let row_index = self.selected_row_index()?;
        let row = self.page.as_ref()?.rows.get(row_index)?.clone();

        Some((row_index, row))
    }

    fn clear_selection(&self) {
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
    }

    pub(super) fn can_delete_rows(&self) -> bool {
        self.page
            .as_ref()
            .is_some_and(|page| page.object.kind == DatabaseObjectKind::Table)
            && self
                .available_columns
                .iter()
                .any(|column| column.is_primary_key)
            && self
                .available_columns
                .iter()
                .filter(|column| column.is_primary_key)
                .all(|column| column.is_editable_value_type())
    }

    pub(super) fn can_delete_selected_row(&self) -> bool {
        !self.is_loading && self.can_delete_rows() && self.selected_row().is_some()
    }

    fn status_icon_name(&self) -> &'static str {
        if self.is_error {
            "dialog-error-symbolic"
        } else if self.is_loading {
            "view-refresh-symbolic"
        } else {
            "table-symbolic"
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

    fn open_export_dialog(&self, root: &gtk::Box, sender: &ComponentSender<Self>) {
        if self.pool.is_none() || self.object.is_none() {
            return;
        }

        let parent = root.root().and_downcast::<gtk::Window>();
        let initial_name = self
            .object
            .as_ref()
            .map(csv_filename_for_object)
            .unwrap_or_else(|| "table.csv".to_string());
        let sender = sender.clone();

        show_csv_export_options_dialog(parent.clone().as_ref(), move |options| {
            let parent = parent.clone();
            let sender = sender.clone();

            show_csv_save_dialog(parent.as_ref(), initial_name, move |path| {
                sender.input(TableBrowserMsg::ExportCsvConfirmed { options, path });
            });
        });
    }

    fn export_csv(&self, options: CsvExportOptions, path: PathBuf, sender: &ComponentSender<Self>) {
        let Some(pool) = self.pool.clone() else {
            return;
        };
        let Some(object) = self.object.clone() else {
            return;
        };

        let filters = self.active_filters.clone();
        let sort = self.sort.clone();

        sender.oneshot_command(async move {
            let result = async {
                let page = crate::db::browser::export_table_page(
                    &pool,
                    &object,
                    &filters,
                    sort.as_ref(),
                    options,
                )
                .await
                .map_err(|error| error.to_string())?;
                let csv = csv_export::table_page(&page, options);

                std::fs::write(path, csv).map_err(|error| error.to_string())
            }
            .await;

            TableBrowserCommandOutput::CsvExported(result)
        });
    }
}

fn close_popover(popover: &mut Option<gtk::Popover>) {
    if let Some(popover) = popover.take() {
        popover.popdown();

        if popover.parent().is_some() {
            popover.unparent();
        }
    }
}

fn copy_text_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

fn context_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    let edit_section = gio::Menu::new();
    edit_section.append(Some(&gettext("Edit Value")), Some("browser.edit-cell"));
    menu.append_section(None, &edit_section);

    let copy_section = gio::Menu::new();
    copy_section.append(Some(&gettext("Copy Cell")), Some("browser.copy-cell"));
    copy_section.append(Some(&gettext("Copy Row")), Some("browser.copy-row"));
    copy_section.append(Some(&gettext("Copy Column")), Some("browser.copy-column"));
    copy_section.append(
        Some(&gettext("Copy Displayed Page")),
        Some("browser.copy-page"),
    );
    copy_section.append(Some(&gettext("Export CSV...")), Some("browser.export-csv"));
    menu.append_section(None, &copy_section);

    let destructive_section = gio::Menu::new();
    destructive_section.append(Some(&gettext("Delete Row...")), Some("browser.delete-row"));
    menu.append_section(None, &destructive_section);

    menu
}

fn delete_key_controller(sender: &ComponentSender<TableBrowser>) -> gtk::EventControllerKey {
    let controller = gtk::EventControllerKey::new();
    controller.connect_key_pressed({
        let sender = sender.clone();

        move |_, key, _, state| {
            let shortcut_modifiers = gtk::gdk::ModifierType::CONTROL_MASK
                | gtk::gdk::ModifierType::SHIFT_MASK
                | gtk::gdk::ModifierType::ALT_MASK
                | gtk::gdk::ModifierType::SUPER_MASK
                | gtk::gdk::ModifierType::META_MASK;

            if key == gtk::gdk::Key::Delete && !state.intersects(shortcut_modifiers) {
                sender.input(TableBrowserMsg::DeleteSelectedRowRequested);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });

    controller
}

fn context_action_group(
    copy_target: Rc<Cell<Option<CopyTarget>>>,
    edit_target: Rc<RefCell<Option<EditTarget>>>,
    sender: ComponentSender<TableBrowser>,
) -> (gio::SimpleActionGroup, gio::SimpleAction, gio::SimpleAction) {
    let action_group = gio::SimpleActionGroup::new();
    let edit_action = gio::SimpleAction::new("edit-cell", None);
    edit_action.set_enabled(false);
    edit_action.connect_activate({
        let edit_target = edit_target.clone();
        let sender = sender.clone();

        move |_, _| {
            let Some(target) = edit_target.borrow().clone() else {
                return;
            };

            sender.input(TableBrowserMsg::EditCellFromMenu {
                anchor: target.anchor,
                row_index: target.row_index,
                column_index: target.column_index,
            });
        }
    });
    action_group.add_action(&edit_action);

    let delete_action = gio::SimpleAction::new("delete-row", None);
    delete_action.set_enabled(false);
    delete_action.connect_activate({
        let sender = sender.clone();

        move |_, _| {
            sender.input(TableBrowserMsg::DeleteSelectedRowRequested);
        }
    });
    action_group.add_action(&delete_action);

    let actions = [
        "copy-cell",
        "copy-row",
        "copy-column",
        "copy-page",
        "export-csv",
    ];

    for name in actions {
        let simple_action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        let copy_target = copy_target.clone();

        simple_action.connect_activate(move |_, _| {
            if name == "export-csv" {
                sender.input(TableBrowserMsg::ExportCsvRequested);
                return;
            }

            let Some(target) = copy_target.get() else {
                return;
            };

            sender.input(match name {
                "copy-cell" => target.cell_message(),
                "copy-row" => target.row_message(),
                "copy-column" => target.column_message(),
                "copy-page" => TableBrowserMsg::CopyPage,
                _ => return,
            });
        });

        action_group.add_action(&simple_action);
    }

    (action_group, edit_action, delete_action)
}

fn csv_filename_for_object(object: &DatabaseObject) -> String {
    let schema = sanitize_filename_part(&object.schema);
    let name = sanitize_filename_part(&object.name);

    format!("{schema}.{name}.csv")
}

fn sanitize_filename_part(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character => character,
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
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
