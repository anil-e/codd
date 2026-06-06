mod columns;
mod sections;

use futures_util::future::{AbortHandle, Abortable};
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;
use sqlx::PgPool;
use std::cell::RefCell;
use std::rc::Rc;

use crate::db;
use crate::models::database_object::DatabaseObject;
use crate::models::structure_action::{StructureActionKind, StructureActionTarget};
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
    context_target: Rc<RefCell<Option<StructureActionTarget>>>,
    context_popover: gtk::PopoverMenu,
    rename_action: gio::SimpleAction,
    drop_action: gio::SimpleAction,
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
    CopyNameRequested,
    RenameRequested,
    DropRequested,
}

#[derive(Debug)]
pub enum TableStructureOutput {
    Copied { text: String, message: String },
    RenameRequested(StructureActionTarget),
    DropRequested(StructureActionTarget),
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
    type Output = TableStructureOutput;
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

        let context_target = Rc::new(RefCell::new(None));
        let rename_action = gio::SimpleAction::new("rename", None);
        let drop_action = gio::SimpleAction::new("drop", None);
        let context_popover = gtk::PopoverMenu::from_model(Some(&structure_context_menu()));
        context_popover.set_has_arrow(false);

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
            context_target: context_target.clone(),
            context_popover,
            rename_action,
            drop_action,
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
        };

        let widgets = view_output!();
        model.context_popover.set_parent(&widgets.stack);
        widgets
            .stack
            .insert_action_group("structure", Some(&context_action_group(&model, &sender)));
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

            TableStructureMsg::CopyNameRequested => {
                if let Some(target) = self.context_target.borrow().clone() {
                    let _ = sender.output(TableStructureOutput::Copied {
                        text: target.name.clone(),
                        message: copied_message(target.kind),
                    });
                }
                self.close_context_menu();
            }

            TableStructureMsg::RenameRequested => {
                if let Some(target) = self.context_target.borrow().clone()
                    && target.editable
                {
                    let _ = sender.output(TableStructureOutput::RenameRequested(target));
                }
                self.close_context_menu();
            }

            TableStructureMsg::DropRequested => {
                if let Some(target) = self.context_target.borrow().clone()
                    && target.editable
                {
                    let _ = sender.output(TableStructureOutput::DropRequested(target));
                }
                self.close_context_menu();
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

        if self.context_popover.parent().is_some() {
            self.context_popover.unparent();
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
            let context = StructureContextMenu {
                popover: self.context_popover.clone(),
                target: self.context_target.clone(),
                rename_action: self.rename_action.clone(),
                drop_action: self.drop_action.clone(),
            };

            append_columns_section(
                &self.structure_box,
                structure,
                self.style_manager.is_dark(),
                context.clone(),
            );
            append_indexes_section(&self.structure_box, structure, context.clone());
            append_constraints_section(&self.structure_box, structure, context.clone());
            append_foreign_keys_section(&self.structure_box, structure, context.clone());
            append_triggers_section(&self.structure_box, structure, context);
        }
    }

    fn close_context_menu(&self) {
        self.context_popover.popdown();
        *self.context_target.borrow_mut() = None;
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
}

#[derive(Clone)]
pub(super) struct StructureContextMenu {
    popover: gtk::PopoverMenu,
    target: Rc<RefCell<Option<StructureActionTarget>>>,
    rename_action: gio::SimpleAction,
    drop_action: gio::SimpleAction,
}

impl StructureContextMenu {
    pub(super) fn attach<W>(&self, widget: &W, target: StructureActionTarget)
    where
        W: IsA<gtk::Widget> + Clone + 'static,
    {
        let context_click = gtk::GestureClick::new();
        context_click.set_button(gtk::gdk::BUTTON_SECONDARY);
        context_click.set_propagation_phase(gtk::PropagationPhase::Capture);

        let anchor = widget.clone();
        let context = self.clone();
        context_click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);

            context.rename_action.set_enabled(target.editable);
            context.drop_action.set_enabled(target.editable);
            *context.target.borrow_mut() = Some(target.clone());
            show_context_menu(&anchor, &context.popover, x, y);
        });

        widget.add_controller(context_click);
    }
}

fn structure_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some(&gettext("Copy Name")), Some("structure.copy-name"));
    menu.append(Some(&gettext("Rename...")), Some("structure.rename"));
    menu.append(Some(&gettext("Drop...")), Some("structure.drop"));
    menu
}

fn copied_message(kind: StructureActionKind) -> String {
    match kind {
        StructureActionKind::Column => gettext("Column name copied."),
        StructureActionKind::Index => gettext("Index name copied."),
        StructureActionKind::Constraint => gettext("Constraint name copied."),
        StructureActionKind::ForeignKey => gettext("Foreign key name copied."),
        StructureActionKind::Trigger => gettext("Trigger name copied."),
    }
}

fn context_action_group(
    model: &TableStructureView,
    sender: &ComponentSender<TableStructureView>,
) -> gio::SimpleActionGroup {
    let action_group = gio::SimpleActionGroup::new();

    for name in ["copy-name", "rename", "drop"] {
        let action = match name {
            "rename" => model.rename_action.clone(),
            "drop" => model.drop_action.clone(),
            _ => gio::SimpleAction::new(name, None),
        };
        let sender = sender.clone();

        action.connect_activate(move |_, _| {
            sender.input(match name {
                "copy-name" => TableStructureMsg::CopyNameRequested,
                "rename" => TableStructureMsg::RenameRequested,
                "drop" => TableStructureMsg::DropRequested,
                _ => unreachable!(),
            });
        });

        action_group.add_action(&action);
    }

    action_group
}

fn show_context_menu<W>(anchor: &W, popover: &gtk::PopoverMenu, x: f64, y: f64)
where
    W: IsA<gtk::Widget>,
{
    if let Some(parent) = popover.parent()
        && let Some(point) =
            anchor.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
    {
        let rect = gtk::gdk::Rectangle::new(point.x() as i32, point.y() as i32, 1, 1);
        popover.set_pointing_to(Some(&rect));
    }

    popover.popup();
}

fn set_stack_child(widgets: &TableStructureViewWidgets, has_structure: bool) {
    widgets
        .stack
        .set_visible_child_name(if has_structure { "structure" } else { "status" });
}
