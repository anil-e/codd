use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use crate::models::table_script::TableScriptKind;
use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;
use std::collections::BTreeMap;

pub struct ObjectSidebar {
    objects: Vec<DatabaseObject>,
    search_text: String,
    is_loading: bool,
    status_text: String,
    selected_object: Option<String>,
    object_rows: Vec<(String, adw::ActionRow)>,
    context_menu_popovers: Vec<gtk::PopoverMenu>,
}

#[derive(Debug)]
pub enum ObjectSidebarMsg {
    Loading,
    SetObjects(Vec<DatabaseObject>),
    SetError(String),
    SetSelectedObject(Option<DatabaseObject>),
    SearchChanged(String),
    FocusSearch,
    ObjectSelected(DatabaseObject),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectAction {
    Rename,
    Truncate,
    Delete,
}

#[derive(Debug)]
pub enum ObjectSidebarOutput {
    OpenObject(DatabaseObject),
    CopyText {
        text: String,
        message: String,
    },
    ObjectAction {
        object: DatabaseObject,
        action: ObjectAction,
    },
    TableScriptRequested {
        object: DatabaseObject,
        kind: TableScriptKind,
    },
}

#[relm4::component(pub)]
impl Component for ObjectSidebar {
    type Init = ();
    type Input = ObjectSidebarMsg;
    type Output = ObjectSidebarOutput;
    type CommandOutput = ();

    view! {
        gtk::Stack {
            add_css_class: "object-sidebar",

            add_named[Some("status")] = &adw::StatusPage {
                #[watch]
                set_icon_name: Some(model.status_icon_name()),
                #[watch]
                set_title: &model.status_title(),
                #[watch]
                set_description: model.status_description().as_deref(),

                #[wrap(Some)]
                set_child = &gtk::Spinner {
                    #[watch]
                    set_visible: model.is_loading,
                    #[watch]
                    set_spinning: model.is_loading,
                },
            },

            add_named[Some("list")] = &gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_spacing: 0,

                adw::Clamp {
                    set_maximum_size: 520,
                    set_margin_top: 12,
                    set_margin_start: 12,
                    set_margin_end: 12,

                    #[name = "search_entry"]
                    gtk::SearchEntry {
                        set_placeholder_text: Some(&gettext("Search objects")),
                        connect_search_changed[sender] => move |entry| {
                            sender.input(ObjectSidebarMsg::SearchChanged(entry.text().to_string()));
                        },
                    },
                },

                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_policy: (gtk::PolicyType::Never, gtk::PolicyType::Automatic),

                    adw::Clamp {
                        set_maximum_size: 520,
                        set_margin_top: 12,
                        set_margin_bottom: 16,
                        set_margin_start: 12,
                        set_margin_end: 12,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,

                            #[name = "empty_filter_label"]
                            gtk::Label {
                                set_label: &gettext("No objects match your search."),
                                add_css_class: "dim-label",
                                set_halign: gtk::Align::Center,
                                set_margin_top: 12,
                                #[watch]
                                set_visible: model.has_active_search() && !model.has_search_results(),
                            },

                            #[name = "schema_list"]
                            gtk::ListBox {
                                set_selection_mode: gtk::SelectionMode::None,
                                add_css_class: "boxed-list",
                                #[watch]
                                set_visible: model.has_search_results(),
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
        let model = ObjectSidebar {
            objects: Vec::new(),
            search_text: String::new(),
            is_loading: false,
            status_text: gettext("No connection"),
            selected_object: None,
            object_rows: Vec::new(),
            context_menu_popovers: Vec::new(),
        };
        let widgets = view_output!();
        root.set_visible_child_name(model.visible_child_name());

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
            ObjectSidebarMsg::Loading => {
                self.clear_rendered_rows();
                self.is_loading = true;
                self.status_text = gettext("Loading schema...");
                self.objects.clear();
                self.search_text.clear();
                widgets.search_entry.set_text("");
                self.selected_object = None;
                clear_list(&widgets.schema_list);
            }

            ObjectSidebarMsg::SetObjects(objects) => {
                self.is_loading = false;
                self.status_text = if objects.is_empty() {
                    gettext("No tables or views found")
                } else {
                    String::new()
                };
                self.objects = objects;
                self.render_lists(widgets, &sender);
            }

            ObjectSidebarMsg::SetError(error) => {
                self.clear_rendered_rows();
                self.is_loading = false;
                self.objects.clear();
                self.status_text = error;
                self.search_text.clear();
                widgets.search_entry.set_text("");
                self.selected_object = None;
                clear_list(&widgets.schema_list);
            }

            ObjectSidebarMsg::SetSelectedObject(object) => {
                self.selected_object = object.as_ref().map(object_key);
                sync_selected_rows(&self.object_rows, self.selected_object.as_deref());
            }

            ObjectSidebarMsg::SearchChanged(text) => {
                self.search_text = text;
                self.render_lists(widgets, &sender);
            }

            ObjectSidebarMsg::FocusSearch => {
                widgets.search_entry.grab_focus();
                widgets.search_entry.select_region(0, -1);
            }

            ObjectSidebarMsg::ObjectSelected(object) => {
                self.selected_object = Some(object_key(&object));
                sync_selected_rows(&self.object_rows, self.selected_object.as_deref());
                sender.output(ObjectSidebarOutput::OpenObject(object)).ok();
            }
        }

        root.set_visible_child_name(self.visible_child_name());
        self.update_view(widgets, sender);
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        self.clear_rendered_rows();
    }
}

impl ObjectSidebar {
    fn render_lists(&mut self, widgets: &ObjectSidebarWidgets, sender: &ComponentSender<Self>) {
        self.clear_rendered_rows();
        clear_list(&widgets.schema_list);

        let mut object_rows = Vec::new();
        let mut context_menu_popovers = Vec::new();
        let has_active_search = self.has_active_search();
        let selected_object = self.selected_object.as_deref();

        for (schema, objects) in objects_by_schema(self.filtered_objects()) {
            let row = build_schema_row(
                &schema,
                &objects,
                selected_object,
                has_active_search,
                sender,
                &mut object_rows,
                &mut context_menu_popovers,
            );
            widgets.schema_list.append(&row);
        }

        self.object_rows = object_rows;
        self.context_menu_popovers = context_menu_popovers;
    }

    fn clear_rendered_rows(&mut self) {
        self.clear_context_menu();
        self.object_rows.clear();
    }

    fn clear_context_menu(&mut self) {
        for popover in self.context_menu_popovers.drain(..) {
            popover.popdown();
            popover.unparent();
        }
    }

    fn filtered_objects(&self) -> Vec<&DatabaseObject> {
        let search = self.search_text.trim().to_lowercase();
        if search.is_empty() {
            return self.objects.iter().collect();
        }

        self.objects
            .iter()
            .filter(|object| object.name.to_lowercase().contains(&search))
            .collect()
    }

    fn has_active_search(&self) -> bool {
        !self.search_text.trim().is_empty()
    }

    fn has_search_results(&self) -> bool {
        let search = self.search_text.trim().to_lowercase();
        if search.is_empty() {
            return !self.objects.is_empty();
        }

        self.objects
            .iter()
            .any(|object| object.name.to_lowercase().contains(&search))
    }
}

fn objects_by_schema(objects: Vec<&DatabaseObject>) -> BTreeMap<String, Vec<&DatabaseObject>> {
    let mut schemas = BTreeMap::<String, Vec<&DatabaseObject>>::new();

    for object in objects {
        schemas
            .entry(object.schema.clone())
            .or_default()
            .push(object);
    }

    for objects in schemas.values_mut() {
        objects.sort_by(|a, b| {
            let kind_order = object_kind_order(&a.kind).cmp(&object_kind_order(&b.kind));
            kind_order.then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    schemas
}

fn build_schema_row(
    schema: &str,
    objects: &[&DatabaseObject],
    selected_key: Option<&str>,
    has_active_search: bool,
    sender: &ComponentSender<ObjectSidebar>,
    object_rows: &mut Vec<(String, adw::ActionRow)>,
    context_menu_popovers: &mut Vec<gtk::PopoverMenu>,
) -> adw::ExpanderRow {
    let object_count = u32::try_from(objects.len()).unwrap_or(u32::MAX);
    let row = adw::ExpanderRow::builder()
        .title(schema)
        .subtitle(format!(
            "{} {}",
            objects.len(),
            ngettext("object", "objects", object_count)
        ))
        .expanded(
            has_active_search
                || schema == "public"
                || schema_has_selected_object(objects, selected_key),
        )
        .build();

    row.add_prefix(
        &gtk::Image::builder()
            .icon_name("folder-symbolic")
            .pixel_size(16)
            .build(),
    );

    for object in objects {
        row.add_row(&build_object_row(
            object,
            selected_key,
            sender,
            object_rows,
            context_menu_popovers,
        ));
    }

    row
}

fn schema_has_selected_object(objects: &[&DatabaseObject], selected_key: Option<&str>) -> bool {
    let Some(selected_key) = selected_key else {
        return false;
    };

    objects
        .iter()
        .any(|object| object_key(object) == selected_key)
}

fn object_kind_order(kind: &DatabaseObjectKind) -> u8 {
    match kind {
        DatabaseObjectKind::Table => 0,
        DatabaseObjectKind::View => 1,
    }
}

impl ObjectSidebar {
    fn visible_child_name(&self) -> &'static str {
        if self.is_loading || self.objects.is_empty() {
            "status"
        } else {
            "list"
        }
    }

    fn status_title(&self) -> String {
        if self.is_loading {
            gettext("Loading schema")
        } else if self.status_text.is_empty() {
            gettext("No objects available")
        } else {
            self.status_text.clone()
        }
    }

    fn status_description(&self) -> Option<String> {
        if self.is_loading {
            Some(gettext(
                "Fetching tables and views for the current connection.",
            ))
        } else if self.status_text == gettext("No connection") {
            Some(gettext("Connect to PostgreSQL to browse database objects."))
        } else if self.status_text == gettext("No tables or views found") {
            Some(gettext(
                "The selected database does not currently expose tables or views.",
            ))
        } else {
            None
        }
    }

    fn status_icon_name(&self) -> &'static str {
        if self.is_loading {
            "view-refresh-symbolic"
        } else if self.status_text == gettext("No connection") {
            "network-server-symbolic"
        } else if self.status_text == gettext("No tables or views found") {
            "view-list-symbolic"
        } else {
            "dialog-error-symbolic"
        }
    }
}

fn build_object_row(
    object: &DatabaseObject,
    selected_key: Option<&str>,
    sender: &ComponentSender<ObjectSidebar>,
    object_rows: &mut Vec<(String, adw::ActionRow)>,
    context_menu_popovers: &mut Vec<gtk::PopoverMenu>,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&object.name)
        .activatable(true)
        .build();

    let icon_name = match object.kind {
        DatabaseObjectKind::Table => "table-symbolic",
        DatabaseObjectKind::View => "view-list-symbolic",
    };

    row.add_prefix(
        &gtk::Image::builder()
            .icon_name(icon_name)
            .pixel_size(16)
            .build(),
    );

    let key = object_key(object);
    if Some(key.as_str()) == selected_key {
        row.add_css_class("accent");
    }

    object_rows.push((key, row.clone()));

    let popover = gtk::PopoverMenu::from_model(Some(&object_menu(&object.kind)));
    popover.set_has_arrow(false);
    popover.set_parent(&row);
    context_menu_popovers.push(popover.clone());

    row.insert_action_group(
        "object",
        Some(&object_action_group(object.clone(), sender.clone())),
    );

    let context_click = gtk::GestureClick::new();
    context_click.set_button(gtk::gdk::BUTTON_SECONDARY);
    context_click.connect_pressed({
        let popover = popover.clone();

        move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            popover.popup();
        }
    });

    row.add_controller(context_click);

    let selected = object.clone();
    let sender = sender.clone();
    row.connect_activated(move |_| {
        sender.input(ObjectSidebarMsg::ObjectSelected(selected.clone()));
    });

    row
}

fn object_action_group(
    object: DatabaseObject,
    sender: ComponentSender<ObjectSidebar>,
) -> gtk::gio::SimpleActionGroup {
    let action_group = gtk::gio::SimpleActionGroup::new();
    let copy_actions = [
        (
            "copy-name",
            object.name.clone(),
            object_copy_message(&object.kind, false),
        ),
        (
            "copy-qualified-name",
            object.qualified_name(),
            object_copy_message(&object.kind, true),
        ),
    ];

    for (name, text, message) in copy_actions {
        let simple_action = gtk::gio::SimpleAction::new(name, None);
        let sender = sender.clone();

        simple_action.connect_activate(move |_, _| {
            sender
                .output(ObjectSidebarOutput::CopyText {
                    text: text.clone(),
                    message: message.clone(),
                })
                .ok();
        });

        action_group.add_action(&simple_action);
    }

    for (name, action) in [
        ("rename", ObjectAction::Rename),
        ("truncate", ObjectAction::Truncate),
        ("delete", ObjectAction::Delete),
    ] {
        let simple_action = gtk::gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        let object = object.clone();

        simple_action.connect_activate(move |_, _| {
            sender
                .output(ObjectSidebarOutput::ObjectAction {
                    object: object.clone(),
                    action,
                })
                .ok();
        });

        action_group.add_action(&simple_action);
    }

    if object.kind == DatabaseObjectKind::Table {
        for (name, kind) in [
            ("script-create", TableScriptKind::Create),
            ("script-select", TableScriptKind::Select),
            ("script-insert", TableScriptKind::Insert),
            ("script-update", TableScriptKind::Update),
            ("script-delete", TableScriptKind::Delete),
        ] {
            let simple_action = gtk::gio::SimpleAction::new(name, None);
            let sender = sender.clone();
            let object = object.clone();

            simple_action.connect_activate(move |_, _| {
                sender
                    .output(ObjectSidebarOutput::TableScriptRequested {
                        object: object.clone(),
                        kind,
                    })
                    .ok();
            });

            action_group.add_action(&simple_action);
        }
    }

    action_group
}

fn object_menu(kind: &DatabaseObjectKind) -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    let copy_section = gtk::gio::Menu::new();

    copy_section.append(
        Some(&object_copy_label(kind, false)),
        Some("object.copy-name"),
    );
    copy_section.append(
        Some(&object_copy_label(kind, true)),
        Some("object.copy-qualified-name"),
    );

    menu.append_section(None, &copy_section);

    if *kind == DatabaseObjectKind::Table {
        menu.append_submenu(Some(&gettext("Scripts")), &table_scripts_menu());
    }

    let edit_section = gtk::gio::Menu::new();
    edit_section.append(Some(&gettext("Rename...")), Some("object.rename"));
    menu.append_section(None, &edit_section);

    let destructive_section = gtk::gio::Menu::new();
    if *kind == DatabaseObjectKind::Table {
        destructive_section.append(Some(&gettext("Truncate...")), Some("object.truncate"));
    }

    destructive_section.append(Some(&gettext("Delete...")), Some("object.delete"));
    menu.append_section(None, &destructive_section);

    menu
}

fn table_scripts_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    menu.append(
        Some(&gettext("CREATE Script")),
        Some("object.script-create"),
    );
    menu.append(
        Some(&gettext("SELECT Script")),
        Some("object.script-select"),
    );
    menu.append(
        Some(&gettext("INSERT Script")),
        Some("object.script-insert"),
    );
    menu.append(
        Some(&gettext("UPDATE Script")),
        Some("object.script-update"),
    );
    menu.append(
        Some(&gettext("DELETE Script")),
        Some("object.script-delete"),
    );
    menu
}

fn object_copy_label(kind: &DatabaseObjectKind, qualified: bool) -> String {
    match (kind, qualified) {
        (DatabaseObjectKind::Table, false) => gettext("Copy Table Name"),
        (DatabaseObjectKind::Table, true) => gettext("Copy Qualified Table Name"),
        (DatabaseObjectKind::View, false) => gettext("Copy View Name"),
        (DatabaseObjectKind::View, true) => gettext("Copy Qualified View Name"),
    }
}

fn object_copy_message(kind: &DatabaseObjectKind, qualified: bool) -> String {
    match (kind, qualified) {
        (DatabaseObjectKind::Table, false) => gettext("Table name copied."),
        (DatabaseObjectKind::Table, true) => gettext("Qualified table name copied."),
        (DatabaseObjectKind::View, false) => gettext("View name copied."),
        (DatabaseObjectKind::View, true) => gettext("Qualified view name copied."),
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
}

fn object_key(object: &DatabaseObject) -> String {
    let kind = match object.kind {
        DatabaseObjectKind::Table => "table-symbolic",
        DatabaseObjectKind::View => "view",
    };

    format!("{kind}\u{1f}{}\u{1f}{}", object.schema, object.name)
}

fn sync_selected_rows(rows: &[(String, adw::ActionRow)], selected_key: Option<&str>) {
    for (key, row) in rows {
        if Some(key.as_str()) == selected_key {
            row.add_css_class("accent");
        } else {
            remove_accent_class(row);
        }
    }
}

fn remove_accent_class(row: &adw::ActionRow) {
    if row.has_css_class("accent") {
        row.remove_css_class("accent");
    }
}
