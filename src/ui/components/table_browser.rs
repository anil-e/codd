use futures_util::future::{AbortHandle, Abortable};
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
    ColumnTypeGroup, DEFAULT_PAGE_SIZE, PAGE_SIZE_OPTIONS, TableColumn, TablePage,
};
use crate::ui::components::cell_dialog::show_cell_value_dialog;

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
    request_id: u64,
    active_request_id: Option<u64>,
    active_abort_handle: Option<AbortHandle>,
    table_rows: gio::ListStore,
    table_view: gtk::ColumnView,
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
    Refresh,
    PreviousPage,
    NextPage,
    PageSizeChanged(u32),
    PageLoaded {
        id: u64,
        result: Result<TablePage, String>,
    },
    AppearanceChanged,
}

#[relm4::component(pub)]
impl Component for TableBrowser {
    type Init = ();
    type Input = TableBrowserMsg;
    type Output = ();
    type CommandOutput = TableBrowserMsg;

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
                    set_icon_name: "view-refresh-symbolic",
                    set_tooltip_text: Some(&gettext("Refresh")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.object.is_some() && !model.is_loading,
                    connect_clicked => TableBrowserMsg::Refresh,
                },
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
            request_id: 0,
            active_request_id: None,
            active_abort_handle: None,
            table_rows,
            table_view,
            rendered_columns: Vec::new(),
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
        };
        let widgets = view_output!();
        widgets.grid.set_child(Some(&model.table_view));
        set_stack_child(&widgets, false);
        root.set_spacing(0);

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            TableBrowserMsg::Open { pool, object } => {
                self.pool = Some(pool);
                self.object = Some(object);
                self.offset = 0;
                self.load_page(widgets, &sender);
            }

            TableBrowserMsg::Refresh => {
                self.load_page(widgets, &sender);
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

            TableBrowserMsg::PageSizeChanged(page_size) => {
                if self.page_size == page_size {
                    return;
                }

                self.page_size = page_size;
                self.offset = 0;
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
                        self.page = Some(page);
                    }

                    Err(error) => {
                        self.is_error = true;
                        self.status_title = gettext("Loading table failed");
                        self.status_description = Some(error);
                        self.page = None;
                    }
                }

                self.render_table();
                set_stack_child(widgets, self.page.is_some());
            }

            TableBrowserMsg::AppearanceChanged => {
                self.rendered_columns.clear();
                self.render_table();
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
        sender.input(msg);
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        if let Some(handler) = self.dark_notify_handler.take() {
            self.style_manager.disconnect(handler);
        }

        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }
    }
}

impl TableBrowser {
    fn load_page(&mut self, widgets: &TableBrowserWidgets, sender: &ComponentSender<Self>) {
        let (Some(pool), Some(object)) = (self.pool.clone(), self.object.clone()) else {
            return;
        };

        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }

        self.is_loading = true;
        self.is_error = false;
        self.status_title = gettext("Loading rows");
        self.status_description = Some(gettext("Fetching the selected page from PostgreSQL."));
        self.page = None;
        self.render_table();
        set_stack_child(widgets, false);

        let id = self.allocate_request_id();
        let offset = self.offset;
        let page_size = self.page_size;
        self.active_request_id = Some(id);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.active_abort_handle = Some(abort_handle);

        sender.oneshot_command(async move {
            let load = async move {
                db::browser::load_table_page(&pool, &object, offset, page_size)
                    .await
                    .map_err(|error| error.to_string())
            };

            let result = match Abortable::new(load, abort_registration).await {
                Ok(result) => result,
                Err(_) => Err(gettext("Loading cancelled")),
            };

            TableBrowserMsg::PageLoaded { id, result }
        });
    }

    fn allocate_request_id(&mut self) -> u64 {
        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        id
    }

    fn render_table(&mut self) {
        let Some(page) = self.page.clone() else {
            self.table_rows.remove_all();
            clear_columns(&self.table_view);
            self.rendered_columns.clear();
            return;
        };

        self.sync_columns(&page.columns);
        self.table_rows.remove_all();

        for row in &page.rows {
            self.table_rows
                .append(&glib::BoxedAnyObject::new(row.clone()));
        }
    }

    fn sync_columns(&mut self, columns: &[TableColumn]) {
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
            let factory = cell_factory(index, column.type_group, is_dark);
            let title = column.name.clone();
            let view_column = gtk::ColumnViewColumn::new(Some(&title), Some(factory));
            view_column.set_resizable(true);
            view_column.set_expand(index < 3);
            self.table_view.append_column(&view_column);
        }
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
}

fn set_stack_child(widgets: &TableBrowserWidgets, has_page: bool) {
    widgets
        .stack
        .set_visible_child_name(if has_page { "grid" } else { "status" });
}

fn clear_columns(view: &gtk::ColumnView) {
    while let Some(column) = view.columns().item(0) {
        if let Ok(column) = column.downcast::<gtk::ColumnViewColumn>() {
            view.remove_column(&column);
        } else {
            break;
        }
    }
}

fn cell_factory(
    column_index: usize,
    type_group: ColumnTypeGroup,
    is_dark: bool,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let color = type_group_color(type_group, is_dark);

    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let label = gtk::Label::builder()
            .xalign(0.0)
            .selectable(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .lines(1)
            .width_chars(12)
            .max_width_chars(28)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();

        label.add_css_class("query-cell");
        label.set_use_markup(color.is_some());

        label.add_controller({
            let gesture = gtk::GestureClick::new();
            gesture.connect_pressed(move |gesture, press_count, _, _| {
                if press_count == 2
                    && let Some(widget) = gesture.widget()
                    && let Ok(label) = widget.downcast::<gtk::Label>()
                    && let Some(full_value) = label.tooltip_text()
                {
                    show_cell_value_dialog(&label, &full_value);
                }
            });
            gesture
        });

        list_item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Some(label) = list_item.child().and_downcast::<gtk::Label>() else {
            return;
        };
        let Ok(row) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };

        let row = row.borrow::<Vec<String>>();
        let value = row.get(column_index).map_or("", String::as_str);
        if let Some(color) = color {
            let escaped_value = glib::markup_escape_text(value);
            label.set_markup(&format!(
                "<span foreground=\"{color}\">{escaped_value}</span>"
            ));
        } else {
            label.set_label(value);
        }
        label.set_tooltip_text(Some(value));
    });

    factory
}

fn type_group_color(type_group: ColumnTypeGroup, is_dark: bool) -> Option<&'static str> {
    match (type_group, is_dark) {
        (ColumnTypeGroup::Boolean, false) => Some("#1f9d55"),
        (ColumnTypeGroup::Boolean, true) => Some("#4fd785"),
        (ColumnTypeGroup::Binary, false) => Some("#8b5cf6"),
        (ColumnTypeGroup::Binary, true) => Some("#b89cff"),
        (ColumnTypeGroup::DateTime, false) => Some("#0f7abf"),
        (ColumnTypeGroup::DateTime, true) => Some("#66c2ff"),
        (ColumnTypeGroup::Json, false) => Some("#d97706"),
        (ColumnTypeGroup::Json, true) => Some("#ffb15f"),
        (ColumnTypeGroup::Numeric, false) => Some("#6d28d9"),
        (ColumnTypeGroup::Numeric, true) => Some("#c69cff"),
        (ColumnTypeGroup::Text, false) => Some("#0057b7"),
        (ColumnTypeGroup::Text, true) => Some("#79b8ff"),
        (ColumnTypeGroup::Other, _) => None,
    }
}

fn page_size_model() -> gtk::StringList {
    let labels = PAGE_SIZE_OPTIONS
        .iter()
        .map(|page_size| page_size.to_string())
        .collect::<Vec<_>>();
    let borrowed = labels.iter().map(String::as_str).collect::<Vec<_>>();

    gtk::StringList::new(&borrowed)
}
