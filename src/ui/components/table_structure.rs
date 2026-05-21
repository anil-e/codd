mod columns;
mod sections;

use futures_util::future::{AbortHandle, Abortable};
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::models::database_object::DatabaseObject;
use crate::models::table_structure::TableStructure;

use columns::append_columns_section;
use sections::{
    append_constraints_section, append_foreign_keys_section, append_indexes_section,
    append_triggers_section, clear_box,
};

pub struct TableStructureView {
    pool: Option<PgPool>,
    object: Option<DatabaseObject>,
    structure: Option<TableStructure>,
    is_loading: bool,
    is_error: bool,
    status_title: String,
    status_description: Option<String>,
    request_id: u64,
    active_request_id: Option<u64>,
    active_abort_handle: Option<AbortHandle>,
    structure_box: gtk::Box,
    style_manager: adw::StyleManager,
    dark_notify_handler: Option<glib::SignalHandlerId>,
}

#[derive(Debug)]
pub enum TableStructureMsg {
    Open {
        pool: PgPool,
        object: DatabaseObject,
    },
    Refresh,
    StructureLoaded {
        id: u64,
        result: Result<TableStructure, String>,
    },
    AppearanceChanged,
}

#[derive(Debug)]
pub enum TableStructureCommandOutput {
    StructureLoaded {
        id: u64,
        result: Result<TableStructure, String>,
    },
}

#[relm4::component(pub)]
impl Component for TableStructureView {
    type Init = ();
    type Input = TableStructureMsg;
    type Output = ();
    type CommandOutput = TableStructureCommandOutput;

    view! {
        #[name = "stack"]
        gtk::Stack {
            #[name = "status"]
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

            #[name = "structure_scroller"]
            add_named[Some("structure")] = &gtk::ScrolledWindow {
                set_hexpand: true,
                set_vexpand: true,
                set_policy: (gtk::PolicyType::Automatic, gtk::PolicyType::Automatic),
            },
        }
    }

    fn init(
        _init: Self::Init,
        _root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let structure_box = gtk::Box::new(gtk::Orientation::Vertical, 16);
        structure_box.set_margin_top(12);
        structure_box.set_margin_bottom(12);
        structure_box.set_margin_start(12);
        structure_box.set_margin_end(12);

        let style_manager = adw::StyleManager::default();
        let dark_notify_handler = {
            let sender = sender.clone();
            style_manager.connect_dark_notify(move |_| {
                sender.input(TableStructureMsg::AppearanceChanged);
            })
        };

        let model = TableStructureView {
            pool: None,
            object: None,
            structure: None,
            is_loading: false,
            is_error: false,
            status_title: gettext("Structure"),
            status_description: Some(gettext("Open a table to inspect its columns.")),
            request_id: 0,
            active_request_id: None,
            active_abort_handle: None,
            structure_box,
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
        };

        let widgets = view_output!();
        widgets
            .structure_scroller
            .set_child(Some(&model.structure_box));
        set_stack_child(&widgets, false);

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
            TableStructureMsg::Open { pool, object } => {
                self.pool = Some(pool);
                self.object = Some(object);
                self.load_structure(widgets, &sender);
            }

            TableStructureMsg::Refresh => {
                self.load_structure(widgets, &sender);
            }

            TableStructureMsg::StructureLoaded { id, result } => {
                if self.active_request_id != Some(id) {
                    return;
                }

                self.active_request_id = None;
                self.active_abort_handle = None;
                self.is_loading = false;

                match result {
                    Ok(structure) => {
                        self.is_error = false;
                        self.status_title.clear();
                        self.status_description = None;
                        self.structure = Some(structure);
                    }

                    Err(error) => {
                        self.is_error = true;
                        self.status_title = gettext("Loading structure failed");
                        self.status_description = Some(error);
                        self.structure = None;
                    }
                }

                self.render_structure();
                set_stack_child(widgets, self.structure.is_some());
            }

            TableStructureMsg::AppearanceChanged => {
                self.render_structure();
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
        match msg {
            TableStructureCommandOutput::StructureLoaded { id, result } => {
                sender.input(TableStructureMsg::StructureLoaded { id, result });
            }
        }
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }

        if let Some(handler) = self.dark_notify_handler.take() {
            self.style_manager.disconnect(handler);
        }
    }
}

impl TableStructureView {
    fn load_structure(
        &mut self,
        widgets: &TableStructureViewWidgets,
        sender: &ComponentSender<Self>,
    ) {
        let (Some(pool), Some(object)) = (self.pool.clone(), self.object.clone()) else {
            return;
        };

        if let Some(abort_handle) = self.active_abort_handle.take() {
            abort_handle.abort();
        }

        self.is_loading = true;
        self.is_error = false;
        self.status_title = gettext("Loading structure");
        self.status_description = Some(gettext("Fetching table structure from PostgreSQL."));
        self.structure = None;
        clear_box(&self.structure_box);
        set_stack_child(widgets, false);

        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        self.active_request_id = Some(id);
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.active_abort_handle = Some(abort_handle);

        sender.oneshot_command(async move {
            let load = async move {
                db::structure::load_table_structure(&pool, &object)
                    .await
                    .map_err(|error| error.to_string())
            };

            let result = match Abortable::new(load, abort_registration).await {
                Ok(result) => result,
                Err(_) => Err(gettext("Loading cancelled")),
            };

            TableStructureCommandOutput::StructureLoaded { id, result }
        });
    }

    fn render_structure(&self) {
        clear_box(&self.structure_box);

        if let Some(structure) = &self.structure {
            append_columns_section(&self.structure_box, structure, self.style_manager.is_dark());
            append_indexes_section(&self.structure_box, structure);
            append_constraints_section(&self.structure_box, structure);
            append_foreign_keys_section(&self.structure_box, structure);
            append_triggers_section(&self.structure_box, structure);
        }
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

fn set_stack_child(widgets: &TableStructureViewWidgets, has_structure: bool) {
    widgets
        .stack
        .set_visible_child_name(if has_structure { "structure" } else { "status" });
}
