use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;
use sqlx::PgPool;

use crate::db;
use crate::models::database_object::DatabaseObject;
use crate::models::table_browser::ColumnTypeGroup;
use crate::models::table_structure::{TableStructure, TableStructureColumn};
use crate::ui::components::cell_style;

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
    rows: gio::ListStore,
    columns_view: gtk::ColumnView,
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

            #[name = "columns_scroller"]
            add_named[Some("columns")] = &gtk::ScrolledWindow {
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
        let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
        let columns_view = gtk::ColumnView::new(Some(gtk::NoSelection::new(Some(
            rows.clone().upcast::<gio::ListModel>(),
        ))));
        columns_view.set_vexpand(true);
        columns_view.set_hexpand(true);
        columns_view.set_show_row_separators(true);
        columns_view.set_show_column_separators(true);
        columns_view.add_css_class("data-table");

        for column in structure_columns() {
            columns_view.append_column(&column);
        }

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
            rows,
            columns_view,
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
        };

        let widgets = view_output!();
        widgets
            .columns_scroller
            .set_child(Some(&model.columns_view));
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

                self.render_columns();
                set_stack_child(widgets, self.structure.is_some());
            }

            TableStructureMsg::AppearanceChanged => {
                self.render_columns();
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

        self.is_loading = true;
        self.is_error = false;
        self.status_title = gettext("Loading structure");
        self.status_description = Some(gettext("Fetching table columns from PostgreSQL."));
        self.structure = None;
        self.rows.remove_all();
        set_stack_child(widgets, false);

        let id = self.request_id;
        self.request_id = self.request_id.wrapping_add(1);
        self.active_request_id = Some(id);

        sender.oneshot_command(async move {
            let result = db::structure::load_table_structure(&pool, &object)
                .await
                .map_err(|error| error.to_string());

            TableStructureCommandOutput::StructureLoaded { id, result }
        });
    }

    fn render_columns(&self) {
        self.rows.remove_all();

        if let Some(structure) = &self.structure {
            for column in &structure.columns {
                self.rows.append(&glib::BoxedAnyObject::new(column.clone()));
            }
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

fn structure_columns() -> [gtk::ColumnViewColumn; 5] {
    [
        text_column(&gettext("Name"), |column| column.name.clone(), true, None),
        text_column(
            &gettext("Type"),
            |column| column.data_type.clone(),
            true,
            Some(style_type_label),
        ),
        text_column(
            &gettext("Nullable"),
            |column| {
                if column.is_nullable {
                    gettext("Yes")
                } else {
                    gettext("No")
                }
            },
            false,
            None,
        ),
        text_column(&gettext("Default"), default_label, true, None),
        text_column(&gettext("Key"), key_label, false, None),
    ]
}

fn text_column(
    title: &str,
    value: fn(&TableStructureColumn) -> String,
    expand: bool,
    style: Option<fn(&gtk::Label, &TableStructureColumn)>,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let label = gtk::Label::builder()
            .xalign(0.0)
            .selectable(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .lines(1)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();
        label.add_css_class("query-cell");

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

        let row = row.borrow::<TableStructureColumn>();
        let text = value(&row);
        cell_style::clear_type_classes(&label);

        if let Some(style) = style {
            style(&label, &row);
        }

        label.set_label(&text);
        label.set_tooltip_text(if text.is_empty() { None } else { Some(&text) });
    });

    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_resizable(true);
    column.set_expand(expand);
    column
}

fn style_type_label(label: &gtk::Label, column: &TableStructureColumn) {
    cell_style::apply_type_class(
        label,
        ColumnTypeGroup::from_postgres_type(&column.type_name),
        adw::StyleManager::default().is_dark(),
    );
}

fn default_label(column: &TableStructureColumn) -> String {
    if let Some(identity) = column.identity {
        return format!("Identity ({})", identity.label());
    }

    if column.generated.is_some() {
        return gettext("Generated");
    }

    column.default_expression.clone().unwrap_or_default()
}

fn key_label(column: &TableStructureColumn) -> String {
    if column.is_primary_key {
        gettext("Primary")
    } else {
        String::new()
    }
}

fn set_stack_child(widgets: &TableStructureViewWidgets, has_structure: bool) {
    widgets
        .stack
        .set_visible_child_name(if has_structure { "columns" } else { "status" });
}

#[cfg(test)]
mod tests {
    use crate::models::table_structure::{TableColumnIdentity, TableStructureColumn};

    use super::{default_label, key_label};

    #[test]
    fn default_label_prefers_identity() {
        let column = TableStructureColumn {
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            type_name: "int8".to_string(),
            is_nullable: false,
            default_expression: Some("nextval('example_id_seq'::regclass)".to_string()),
            is_primary_key: true,
            identity: Some(TableColumnIdentity::Always),
            generated: None,
        };

        assert_eq!(default_label(&column), "Identity (Always)");
    }

    #[test]
    fn key_label_marks_primary_key() {
        let column = TableStructureColumn {
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            type_name: "int8".to_string(),
            is_nullable: false,
            default_expression: None,
            is_primary_key: true,
            identity: None,
            generated: None,
        };

        assert_eq!(key_label(&column), "Primary");
    }

    #[test]
    fn key_label_is_empty_for_regular_columns() {
        let column = TableStructureColumn {
            name: "name".to_string(),
            data_type: "text".to_string(),
            type_name: "text".to_string(),
            is_nullable: true,
            default_expression: None,
            is_primary_key: false,
            identity: None,
            generated: None,
        };

        assert_eq!(key_label(&column), "");
    }
}
