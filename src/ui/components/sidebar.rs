use crate::models::database_object::{DatabaseObject, DatabaseObjectKind};
use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;
use std::collections::BTreeMap;

pub struct ObjectSidebar {
    objects: Vec<DatabaseObject>,
    is_loading: bool,
    status_text: String,
    selected_object: Option<String>,
}

#[derive(Debug)]
pub enum ObjectSidebarMsg {
    Loading,
    SetObjects(Vec<DatabaseObject>),
    SetError(String),
    ObjectSelected(DatabaseObject),
}

#[derive(Debug)]
pub enum ObjectSidebarOutput {
    PrepareQuery(String),
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

            add_named[Some("list")] = &gtk::ScrolledWindow {
                set_vexpand: true,
                set_policy: (gtk::PolicyType::Never, gtk::PolicyType::Automatic),

                adw::Clamp {
                    set_maximum_size: 520,
                    set_margin_top: 16,
                    set_margin_bottom: 16,
                    set_margin_start: 12,
                    set_margin_end: 12,

                    #[name = "schema_list"]
                    gtk::ListBox {
                        set_selection_mode: gtk::SelectionMode::None,
                        add_css_class: "boxed-list",
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = ObjectSidebar {
            objects: Vec::new(),
            is_loading: false,
            status_text: gettext("No connection"),
            selected_object: None,
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
                self.is_loading = true;
                self.status_text = gettext("Loading schema...");
                self.objects.clear();
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
                self.is_loading = false;
                self.objects.clear();
                self.status_text = error;
                self.selected_object = None;
                clear_list(&widgets.schema_list);
            }

            ObjectSidebarMsg::ObjectSelected(object) => {
                self.selected_object = Some(object_key(&object));
                self.render_lists(widgets, &sender);
                let _ = sender.output(ObjectSidebarOutput::PrepareQuery(
                    object.select_limit_query(),
                ));
            }
        }

        root.set_visible_child_name(self.visible_child_name());
        self.update_view(widgets, sender);
    }
}

impl ObjectSidebar {
    fn render_lists(&self, widgets: &ObjectSidebarWidgets, sender: &ComponentSender<Self>) {
        clear_list(&widgets.schema_list);

        for (schema, objects) in objects_by_schema(&self.objects) {
            let row = build_schema_row(&schema, &objects, self.selected_object.as_deref(), sender);
            widgets.schema_list.append(&row);
        }
    }
}

fn objects_by_schema(objects: &[DatabaseObject]) -> BTreeMap<String, Vec<DatabaseObject>> {
    let mut schemas = BTreeMap::<String, Vec<DatabaseObject>>::new();

    for object in objects {
        schemas
            .entry(object.schema.clone())
            .or_default()
            .push(object.clone());
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
    objects: &[DatabaseObject],
    selected_key: Option<&str>,
    sender: &ComponentSender<ObjectSidebar>,
) -> adw::ExpanderRow {
    let object_count = u32::try_from(objects.len()).unwrap_or(u32::MAX);
    let row = adw::ExpanderRow::builder()
        .title(schema)
        .subtitle(format!(
            "{} {}",
            objects.len(),
            ngettext("object", "objects", object_count)
        ))
        .expanded(schema == "public" || schema_has_selected_object(objects, selected_key))
        .build();

    row.add_prefix(
        &gtk::Image::builder()
            .icon_name("folder-symbolic")
            .pixel_size(16)
            .build(),
    );

    for object in objects {
        row.add_row(&build_object_row(object, selected_key, sender));
    }

    row
}

fn schema_has_selected_object(objects: &[DatabaseObject], selected_key: Option<&str>) -> bool {
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
            "database-symbolic"
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
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&object.name)
        .activatable(true)
        .build();

    let icon_name = match object.kind {
        DatabaseObjectKind::Table => "table",
        DatabaseObjectKind::View => "view-list",
    };

    row.add_prefix(
        &gtk::Image::builder()
            .icon_name(icon_name)
            .pixel_size(16)
            .build(),
    );

    if Some(object_key(object).as_str()) == selected_key {
        row.add_css_class("accent");
    }

    let selected = object.clone();
    let sender = sender.clone();
    row.connect_activated(move |_| {
        sender.input(ObjectSidebarMsg::ObjectSelected(selected.clone()));
    });

    row
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn object_key(object: &DatabaseObject) -> String {
    let kind = match object.kind {
        DatabaseObjectKind::Table => "table",
        DatabaseObjectKind::View => "view",
    };

    format!("{kind}\u{1f}{}\u{1f}{}", object.schema, object.name)
}
