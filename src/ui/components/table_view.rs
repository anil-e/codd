use gettextrs::gettext;
use libadwaita::{self as adw, prelude::*};
use relm4::prelude::*;
use relm4::{gtk, gtk::gio};
use sqlx::PgPool;

use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::structure_action::StructureActionTarget;
use crate::ui::components::{
    table_browser::{BrowserUiState, TableBrowser, TableBrowserMsg, TableBrowserOutput},
    table_structure::{TableStructureMsg, TableStructureOutput, TableStructureView},
};

pub struct TableView {
    pool: Option<PgPool>,
    object: Option<DatabaseObject>,
    mode: TableViewMode,
    structure_loaded: bool,
    browser_state: BrowserUiState,
    auto_refresh_enabled: bool,
    refresh_seconds: u32,
    refresh_status: String,
    refresh_action: gio::SimpleAction,
    deferred_rename: Option<DatabaseObject>,
    deferred_invalidation: Option<bool>,
    browser: Controller<TableBrowser>,
    structure: Controller<TableStructureView>,
}

#[derive(Debug)]
pub enum TableViewMsg {
    PrepareEditsClose(u64),
    CancelEditsClose(u64),
    CommitEditsClose(u64),
    Open {
        pool: PgPool,
        object: DatabaseObject,
    },
    ObjectRenamed(DatabaseObject),
    Refresh,
    SetAutoRefreshEnabled(bool),
    SetRefreshSeconds(u32),
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
    PendingEditsChanged(bool),
    EditsClosePrepared(u64),
    EditsCloseCancelled(u64),
    EditsCloseCommitted(u64),
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
                    #[watch]
                    set_sensitive: !model.browser_state.pending && !model.browser_state.input_locked,
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
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_state.foreground_busy && !model.browser_state.pending && !model.browser_state.input_locked,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::InsertRow,
                },

                gtk::Button {
                    set_icon_name: "edit-copy-symbolic",
                    set_tooltip_text: Some(&gettext("Duplicate selected row")),
                    add_css_class: "flat",
                    #[watch]
                    set_visible: model.can_insert_rows() && model.mode == TableViewMode::Content && model.browser_state.can_duplicate,
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_state.foreground_busy && model.browser_state.can_duplicate && !model.browser_state.pending && !model.browser_state.input_locked,
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
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_state.foreground_busy && model.browser_state.can_delete && !model.browser_state.pending && !model.browser_state.input_locked,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::DeleteRow,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Show or edit filters")),
                    set_icon_name: "filter-symbolic",
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_state.foreground_busy && !model.browser_state.pending && !model.browser_state.input_locked,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::ToggleFilters,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Export CSV")),
                    set_icon_name: "document-save-symbolic",
                    add_css_class: "flat",
                    #[watch]
                    set_sensitive: model.mode == TableViewMode::Content && !model.browser_state.foreground_busy && !model.browser_state.pending && !model.browser_state.input_locked,
                    #[watch]
                    set_opacity: if model.mode == TableViewMode::Content { 1.0 } else { 0.0 },
                    connect_clicked => TableViewMsg::ExportCsv,
                },

                adw::SplitButton {
                    set_icon_name: "view-refresh-symbolic",
                    set_action_name: Some("table.refresh"),
                    set_dropdown_tooltip: &gettext("Automatic Refresh"),
                    #[watch]
                    set_tooltip_text: Some(&if model.auto_refresh_enabled { model.refresh_status.clone() } else { gettext("Refresh") }),
                    add_css_class: "flat",
                    add_css_class: "table-refresh",
                    #[wrap(Some)]
                    set_popover = &gtk::Popover {
                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,
                            set_width_request: 320,
                            set_margin_top: 6,
                            set_margin_bottom: 6,
                            set_margin_start: 6,
                            set_margin_end: 6,

                            gtk::ListBox {
                                set_selection_mode: gtk::SelectionMode::None,
                                add_css_class: "boxed-list",

                                adw::SwitchRow {
                                    set_title: &gettext("Automatic Refresh"),
                                    #[watch]
                                    set_sensitive: model.mode == TableViewMode::Content,
                                    #[watch]
                                    set_active: model.auto_refresh_enabled,
                                    connect_active_notify[sender] => move |row| {
                                        let _ = sender.input_sender().send(TableViewMsg::SetAutoRefreshEnabled(row.is_active()));
                                    },
                                },

                                adw::SpinRow::with_range(5.0, 3600.0, 1.0) {
                                    set_title: &gettext("Interval (seconds)"),
                                    #[watch]
                                    set_sensitive: model.mode == TableViewMode::Content,
                                    #[watch]
                                    set_value: f64::from(model.refresh_seconds),
                                    connect_value_notify[sender] => move |row| {
                                        let _ = sender.input_sender().send(TableViewMsg::SetRefreshSeconds(row.value() as u32));
                                    },
                                },
                            },

                            gtk::Label {
                                add_css_class: "dim-label",
                                add_css_class: "caption",
                                set_wrap: true,
                                set_max_width_chars: 40,
                                set_xalign: 0.0,
                                #[watch]
                                set_label: &model.refresh_status,
                                #[watch]
                                set_visible: !model.refresh_status.is_empty(),
                            },
                        },
                    },
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
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let browser = TableBrowser::builder()
            .launch(())
            .forward(sender.input_sender(), TableViewMsg::BrowserOutput);
        browser.emit(TableBrowserMsg::SetHeaderVisible(false));
        let structure = TableStructureView::builder()
            .launch(())
            .forward(sender.input_sender(), TableViewMsg::StructureOutput);

        let refresh_action = gio::SimpleAction::new("refresh", None);
        refresh_action.connect_activate({
            let input = sender.input_sender().clone();
            move |_, _| {
                let _ = input.send(TableViewMsg::Refresh);
            }
        });
        let actions = gio::SimpleActionGroup::new();
        actions.add_action(&refresh_action);
        root.insert_action_group("table", Some(&actions));

        let model = TableView {
            pool: None,
            object: None,
            mode: TableViewMode::Content,
            structure_loaded: false,
            browser_state: BrowserUiState::default(),
            auto_refresh_enabled: false,
            refresh_seconds: 5,
            refresh_status: String::new(),
            refresh_action,
            deferred_rename: None,
            deferred_invalidation: None,
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

        let input = sender.input_sender().clone();
        widgets
            .mode_stack
            .connect_visible_child_name_notify(move |stack| {
                let msg = match stack.visible_child_name() {
                    Some(name) if name.as_str() == "structure" => TableViewMsg::ShowStructure,
                    _ => TableViewMsg::ShowContent,
                };

                let _ = input.send(msg);
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
        if self.browser.model().authoritative_changes_blocked() {
            match &msg {
                TableViewMsg::ObjectRenamed(object) => {
                    self.deferred_rename = Some(object.clone());
                    self.browser
                        .emit(TableBrowserMsg::ObjectRenamed(object.clone()));

                    return;
                }

                TableViewMsg::StructureChanged {
                    reset_browser_state,
                } => {
                    self.deferred_invalidation =
                        Some(self.deferred_invalidation.unwrap_or(false) || *reset_browser_state);

                    if *reset_browser_state {
                        self.browser.emit(TableBrowserMsg::SchemaChanged);
                    }

                    return;
                }
                _ => {}
            }
        }

        if (self.has_pending_edits() || self.browser.model().authoritative_changes_blocked())
            && matches!(
                &msg,
                TableViewMsg::Open { .. }
                    | TableViewMsg::ObjectRenamed(_)
                    | TableViewMsg::Refresh
                    | TableViewMsg::StructureChanged { .. }
                    | TableViewMsg::ShowStructure
                    | TableViewMsg::StructureOutput(
                        TableStructureOutput::RenameRequested(_)
                            | TableStructureOutput::DropRequested(_)
                    )
            )
        {
            self.browser.emit(TableBrowserMsg::Refresh);
            widgets
                .mode_stack
                .set_visible_child_name(self.mode.visible_child_name());

            return;
        }

        match msg {
            TableViewMsg::PrepareEditsClose(id) => {
                self.browser.emit(TableBrowserMsg::PrepareEditsClose(id))
            }

            TableViewMsg::CancelEditsClose(id) => {
                self.browser.emit(TableBrowserMsg::CancelEditsClose(id))
            }

            TableViewMsg::CommitEditsClose(id) => {
                self.browser.emit(TableBrowserMsg::CommitEditsClose(id))
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::UiStateChanged(state)) => {
                let pending_changed = self.browser_state.pending != state.pending;
                self.browser_state = state;
                if pending_changed {
                    let _ = sender.output(TableViewOutput::PendingEditsChanged(state.pending));
                }
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::EditsClosePrepared(id)) => {
                let _ = sender.output(TableViewOutput::EditsClosePrepared(id));
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::EditsCloseCancelled(id)) => {
                let _ = sender.output(TableViewOutput::EditsCloseCancelled(id));
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::EditsCloseCommitted(id)) => {
                let _ = sender.output(TableViewOutput::EditsCloseCommitted(id));
            }

            TableViewMsg::Open { pool, object } => {
                self.pool = Some(pool.clone());
                self.object = Some(object.clone());
                self.mode = TableViewMode::Content;
                self.auto_refresh_enabled = false;
                self.refresh_seconds = 5;
                self.refresh_status.clear();
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

            TableViewMsg::SetAutoRefreshEnabled(enabled) => {
                if self.auto_refresh_enabled != enabled {
                    self.auto_refresh_enabled = enabled;
                    self.browser.emit(TableBrowserMsg::SetAutoRefresh(
                        enabled.then_some(self.refresh_seconds),
                    ));
                }
            }

            TableViewMsg::SetRefreshSeconds(seconds) => {
                let seconds = seconds.clamp(5, 3600);
                if self.refresh_seconds != seconds {
                    self.refresh_seconds = seconds;
                    if self.auto_refresh_enabled {
                        self.browser
                            .emit(TableBrowserMsg::SetAutoRefresh(Some(seconds)));
                    }
                }
            }

            TableViewMsg::BrowserOutput(TableBrowserOutput::RefreshStateChanged {
                seconds,
                status,
            }) => {
                self.auto_refresh_enabled = seconds.is_some();
                if let Some(seconds) = seconds {
                    self.refresh_seconds = seconds.clamp(5, 3600);
                }
                self.refresh_status = status;
            }

            TableViewMsg::ReloadContent => {
                self.browser.emit(TableBrowserMsg::ReloadContent);
            }

            TableViewMsg::StructureChanged {
                reset_browser_state,
            } => {
                if reset_browser_state {
                    self.browser.emit(TableBrowserMsg::SchemaChanged);
                } else {
                    self.browser.emit(TableBrowserMsg::ReloadContent);
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
                if self.mode == TableViewMode::Content && self.browser_state.can_duplicate {
                    self.browser
                        .emit(TableBrowserMsg::DuplicateSelectedRowRequested);
                }
            }

            TableViewMsg::DeleteRow => {
                if self.mode == TableViewMode::Content && self.browser_state.can_delete {
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

        if !self.browser.model().authoritative_changes_blocked() {
            if let Some(object) = self.deferred_rename.take() {
                self.object = Some(object.clone());
                self.structure_loaded = false;

                if self.mode == TableViewMode::Structure {
                    self.open_structure();
                }
            }

            if let Some(reset_browser_state) = self.deferred_invalidation.take() {
                if reset_browser_state {
                    if self.structure_loaded && self.can_show_structure() {
                        self.structure.emit(TableStructureMsg::Refresh);
                    }
                } else {
                    sender.input(TableViewMsg::StructureChanged {
                        reset_browser_state: false,
                    });
                }
            }
        }

        self.refresh_action.set_enabled(
            !self.browser_state.pending
                && !self.browser_state.input_locked
                && (self.mode != TableViewMode::Content
                    || (!self.browser_state.foreground_busy && !self.browser_state.is_refreshing)),
        );
        self.update_view(widgets, sender);
    }
}

impl TableView {
    pub fn has_pending_edits(&self) -> bool {
        self.browser.model().has_pending_edits()
    }

    pub fn edits_saving(&self) -> bool {
        self.browser.model().edits_saving()
    }

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
