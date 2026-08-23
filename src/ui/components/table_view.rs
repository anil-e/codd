use gettextrs::gettext;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::structure_action::StructureActionTarget;
use crate::ui::components::{
    table_browser::{TableBrowser, TableBrowserMsg, TableBrowserOutput},
    table_structure::{TableStructureMsg, TableStructureOutput, TableStructureView},
};

pub struct TableView {
    pool: Option<PgPool>,
    object: Option<DatabaseObject>,
    mode: TableViewMode,
    structure_loaded: bool,
    browser_busy: bool,
    can_delete_selected_row: bool,
    can_duplicate_selected_row: bool,
    browser: Controller<TableBrowser>,
    structure: Controller<TableStructureView>,
}

#[derive(Debug)]
pub enum TableViewMsg {
    Open {
        pool: PgPool,
        object: DatabaseObject,
    },
    ObjectRenamed(DatabaseObject),
    Refresh,
    ReloadContent,
    StructureChanged {
        reset_browser_state: bool,
    },
    ExportCsv,
    InsertRow,
    DuplicateRow,
    DeleteRow,
    ShowContent,
    ShowStructure,
    ToggleFilters,
    BrowserOutput(TableBrowserOutput),
    StructureOutput(TableStructureOutput),
}

#[derive(Debug)]
pub enum TableViewOutput {
    Copied(String),
    Exported(String),
    Inserted(String),
    Deleted(String),
    StructureCopied {
        text: String,
        message: String,
    },
    StructureRenameRequested {
        pool: PgPool,
        target: StructureActionTarget,
    },
    StructureDropRequested {
        pool: PgPool,
        target: StructureActionTarget,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableViewMode {
    Content,
    Structure,
}

impl TableViewMode {
    fn visible_child_name(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Structure => "structure",
        }
    }
}

#[relm4::component(pub)]
impl Component for TableView {
    type Init = ();
    type Input = TableViewMsg;
    type Output = TableViewOutput;
    type CommandOutput = ();

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

                #[name = "mode_switcher"]
                gtk::StackSwitcher {
                    set_valign: gtk::Align::Center,
                    set_margin_top: 6,
                    set_margin_bottom: 6,
                    add_css_class: "table-mode-switcher",
                    #[watch]
                    set_visible: model.can_show_structure(),
                },

                gtk::Button {
                    set_icon_name: "list-add-symbolic",
                    set_tooltip_text: Some(&gettext("Insert Row")),
                    add_css_class: "flat",
                    #[watch]
                    set_visible: model.can_insert_rows(),
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_busy,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::InsertRow,
                },

                gtk::Button {
                    set_icon_name: "edit-copy-symbolic",
                    set_tooltip_text: Some(&gettext("Duplicate selected row")),
                    add_css_class: "flat",
                    #[watch]
                    set_visible: model.can_insert_rows() && model.mode == TableViewMode::Content && model.can_duplicate_selected_row,
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_busy && model.can_duplicate_selected_row,
                    connect_clicked => TableViewMsg::DuplicateRow,
                },

                gtk::Button {
                    set_icon_name: "user-trash-symbolic",
                    set_tooltip_text: Some(&gettext("Delete selected row")),
                    add_css_class: "flat",
                    add_css_class: "destructive-action",
                    #[watch]
                    set_visible: model.can_insert_rows(),
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_busy && model.can_delete_selected_row,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::DeleteRow,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Show or edit filters")),
                    set_icon_name: "filter-symbolic",
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_busy,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::ToggleFilters,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Export CSV")),
                    set_icon_name: "document-save-symbolic",
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_busy,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::ExportCsv,
                },

                gtk::Button {
                    set_icon_name: "view-refresh-symbolic",
                    set_tooltip_text: Some(&gettext("Refresh")),
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.mode != TableViewMode::Content || !model.browser_busy,
                    connect_clicked => TableViewMsg::Refresh,
                },
            },

            #[name = "mode_stack"]
            gtk::Stack {
                set_hexpand: true,
                set_vexpand: true,
            },
        }
    }

    fn init(
        _init: Self::Init,
        _root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let browser = TableBrowser::builder()
            .launch(())
            .forward(sender.input_sender(), TableViewMsg::BrowserOutput);
        browser.emit(TableBrowserMsg::SetHeaderVisible(false));
        let structure = TableStructureView::builder()
            .launch(())
            .forward(sender.input_sender(), TableViewMsg::StructureOutput);

        let model = TableView {
            pool: None,
            object: None,
            mode: TableViewMode::Content,
            structure_loaded: false,
            browser_busy: false,
            can_delete_selected_row: false,
            can_duplicate_selected_row: false,
            browser,
            structure,
        };

        let widgets = view_output!();
        widgets
            .mode_stack
            .add_titled(model.browser.widget(), Some("content"), &gettext("Content"));
        widgets.mode_stack.add_titled(
            model.structure.widget(),
            Some("structure"),
            &gettext("Structure"),
        );
        widgets
            .mode_stack
            .set_visible_child_name(model.mode.visible_child_name());
        widgets.mode_switcher.set_stack(Some(&widgets.mode_stack));

        let s = sender.clone();
        widgets
            .mode_stack
            .connect_visible_child_name_notify(move |stack| match stack.visible_child_name() {
                Some(name) if name.as_str() == "structure" => s.input(TableViewMsg::ShowStructure),
                _ => s.input(TableViewMsg::ShowContent),
            });

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
            TableViewMsg::Open { pool, object } => {
                self.pool = Some(pool.clone());
                self.object = Some(object.clone());
                self.mode = TableViewMode::Content;
                self.browser_busy = false;
                self.can_delete_selected_row = false;
                self.can_duplicate_selected_row = false;
                self.structure_loaded = false;
                widgets.mode_stack.set_visible_child_name("content");
                self.browser.emit(TableBrowserMsg::Open { pool, object });
            }

            TableViewMsg::ObjectRenamed(object) => {
                self.object = Some(object.clone());
                self.structure_loaded = false;
                self.browser
                    .emit(TableBrowserMsg::ObjectRenamed(object.clone()));

                if self.mode == TableViewMode::Structure {
                    self.open_structure();
                }
            }

            TableViewMsg::Refresh => match self.mode {
                TableViewMode::Content => self.browser.emit(TableBrowserMsg::Refresh),
                TableViewMode::Structure => self.structure.emit(TableStructureMsg::Refresh),
            },

            TableViewMsg::ReloadContent => {
                self.browser.emit(TableBrowserMsg::Refresh);
            }

            TableViewMsg::StructureChanged {
                reset_browser_state,
            } => {
                if reset_browser_state {
                    self.browser.emit(TableBrowserMsg::SchemaChanged);
                } else {
                    self.browser.emit(TableBrowserMsg::Refresh);
                }

                if self.structure_loaded && self.can_show_structure() {
                    self.structure.emit(TableStructureMsg::Refresh);
                }
            }

            TableViewMsg::ExportCsv => {
                if self.mode == TableViewMode::Content {
                    self.browser.emit(TableBrowserMsg::ExportCsvRequested);
                }
            }

            TableViewMsg::InsertRow => {
                if self.mode == TableViewMode::Content && self.can_insert_rows() {
                    self.browser.emit(TableBrowserMsg::InsertRowRequested);
                }
            }

            TableViewMsg::DuplicateRow => {
                if self.mode == TableViewMode::Content && self.can_duplicate_selected_row {
                    self.browser
                        .emit(TableBrowserMsg::DuplicateSelectedRowRequested);
                }
            }

            TableViewMsg::DeleteRow => {
                if self.mode == TableViewMode::Content && self.can_delete_selected_row {
                    self.browser
                        .emit(TableBrowserMsg::DeleteSelectedRowRequested);
                }
            }

            TableViewMsg::ShowContent => {
                self.mode = TableViewMode::Content;
                widgets.mode_stack.set_visible_child_name("content");
            }

            TableViewMsg::ShowStructure => {
                if !self.can_show_structure() {
                    self.mode = TableViewMode::Content;
                    widgets.mode_stack.set_visible_child_name("content");
                } else {
                    self.mode = TableViewMode::Structure;
                    widgets.mode_stack.set_visible_child_name("structure");
                    self.open_structure();
                }
            }

            TableViewMsg::ToggleFilters => {
                self.browser.emit(TableBrowserMsg::ToggleFilters);
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::Copied(message)) => {
                let _ = sender.output(TableViewOutput::Copied(message));
                return;
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::Exported(message)) => {
                let _ = sender.output(TableViewOutput::Exported(message));
                return;
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::Inserted(message)) => {
                let _ = sender.output(TableViewOutput::Inserted(message));
                return;
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::Deleted(message)) => {
                let _ = sender.output(TableViewOutput::Deleted(message));
                return;
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::BusyChanged(busy)) => {
                self.browser_busy = busy;
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::SelectionChanged {
                can_delete,
                can_duplicate,
            }) => {
                self.can_delete_selected_row = can_delete;
                self.can_duplicate_selected_row = can_duplicate;
            }

            TableViewMsg::StructureOutput(TableStructureOutput::Copied { text, message }) => {
                let _ = sender.output(TableViewOutput::StructureCopied { text, message });
                return;
            }

            TableViewMsg::StructureOutput(TableStructureOutput::RenameRequested(target)) => {
                if let Some(pool) = self.pool.clone() {
                    let _ =
                        sender.output(TableViewOutput::StructureRenameRequested { pool, target });
                }
                return;
            }

            TableViewMsg::StructureOutput(TableStructureOutput::DropRequested(target)) => {
                if let Some(pool) = self.pool.clone() {
                    let _ = sender.output(TableViewOutput::StructureDropRequested { pool, target });
                }
                return;
            }
        }

        self.update_view(widgets, sender);
    }
}

impl TableView {
    fn open_structure(&mut self) {
        if self.structure_loaded || !self.can_show_structure() {
            return;
        }

        let (Some(pool), Some(object)) = (self.pool.clone(), self.object.clone()) else {
            return;
        };

        self.structure_loaded = true;
        self.structure
            .emit(TableStructureMsg::Open { pool, object });
    }

    fn object_title(&self) -> String {
        self.object
            .as_ref()
            .map(|object| object.name.clone())
            .unwrap_or_else(|| gettext("Table"))
    }

    fn can_show_structure(&self) -> bool {
        self.object
            .as_ref()
            .is_some_and(|object| object.kind == DatabaseObjectKind::Table)
    }

    fn can_insert_rows(&self) -> bool {
        self.object
            .as_ref()
            .is_some_and(|object| object.kind == DatabaseObjectKind::Table)
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
}
