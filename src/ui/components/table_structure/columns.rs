use gettextrs::gettext;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};

use crate::models::structure_action::{StructureActionKind, StructureActionTarget};
use crate::models::table_browser::ColumnTypeGroup;
use crate::models::table_structure::{TableStructure, TableStructureColumn};
use crate::ui::components::cell_style;

use super::{StructureContextMenu, sections::section_box};

pub(super) fn append_columns_section(
    container: &gtk::Box,
    structure: &TableStructure,
    is_dark: bool,
    context: StructureContextMenu,
) {
    let section = section_box(&gettext("Columns"), structure.columns.len());

    let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
    for column in &structure.columns {
        rows.append(&glib::BoxedAnyObject::new(column.clone()));
    }

    let columns_view = gtk::ColumnView::new(Some(gtk::NoSelection::new(Some(
        rows.upcast::<gio::ListModel>(),
    ))));
    columns_view.set_hexpand(true);
    columns_view.set_show_row_separators(true);
    columns_view.set_show_column_separators(true);
    columns_view.add_css_class("data-table");

    for column in structure_columns(structure, is_dark, context) {
        columns_view.append_column(&column);
    }

    section.append(&columns_view);
    container.append(&section);
}

fn structure_columns(
    structure: &TableStructure,
    is_dark: bool,
    context: StructureContextMenu,
) -> [gtk::ColumnViewColumn; 5] {
    [
        text_column(
            structure,
            &gettext("Name"),
            |column| column.name.clone(),
            true,
            false,
            is_dark,
            context.clone(),
        ),
        text_column(
            structure,
            &gettext("Type"),
            |column| column.data_type.clone(),
            true,
            true,
            is_dark,
            context.clone(),
        ),
        text_column(
            structure,
            &gettext("Nullable"),
            |column| {
                if column.is_nullable {
                    gettext("Yes")
                } else {
                    gettext("No")
                }
            },
            false,
            false,
            is_dark,
            context.clone(),
        ),
        text_column(
            structure,
            &gettext("Default"),
            default_label,
            true,
            false,
            is_dark,
            context.clone(),
        ),
        text_column(
            structure,
            &gettext("Key"),
            key_label,
            false,
            false,
            is_dark,
            context,
        ),
    ]
}

fn text_column(
    structure: &TableStructure,
    title: &str,
    value: fn(&TableStructureColumn) -> String,
    expand: bool,
    style_type: bool,
    is_dark: bool,
    context: StructureContextMenu,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let table = structure.object.clone();

    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let list_item = list_item.clone();
        let table = table.clone();
        let context = context.clone();

        let label = gtk::Label::builder()
            .xalign(0.0)
            .selectable(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .lines(1)
            .build();
        label.add_css_class("query-cell");
        label.add_css_class("data-cell");
        label.add_controller({
            let gesture = gtk::GestureClick::new();
            let label = label.clone();
            let list_item = list_item.clone();

            gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
            gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            gesture.connect_pressed(move |gesture, _, x, y| {
                gesture.set_state(gtk::EventSequenceState::Claimed);

                if let Some(item) = list_item.item()
                    && let Ok(row) = item.downcast::<glib::BoxedAnyObject>()
                {
                    let row = row.borrow::<TableStructureColumn>();
                    let target = StructureActionTarget::new(
                        table.clone(),
                        StructureActionKind::Column,
                        row.name.clone(),
                        true,
                    );
                    context.rename_action.set_enabled(target.editable);
                    context.drop_action.set_enabled(target.editable);
                    *context.target.borrow_mut() = Some(target);
                    super::show_context_menu(&label, &context.popover, x, y);
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

        let row = row.borrow::<TableStructureColumn>();
        let text = value(&row);
        cell_style::clear_type_classes(&label);

        if style_type {
            cell_style::apply_type_class(
                &label,
                ColumnTypeGroup::from_postgres_type(&row.type_name),
                is_dark,
            );
        }

        label.set_label(&text);
        label.set_tooltip_text(if text.is_empty() { None } else { Some(&text) });
    });

    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_resizable(true);
    column.set_expand(expand);
    column
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
