use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::{self, glib};

use crate::models::table_structure::{
    TableConstraint, TableForeignKey, TableIndex, TableStructure, TableTrigger,
};
use crate::models::{
    database_object::DatabaseObject,
    structure_action::{StructureActionKind, StructureActionTarget},
};

use super::StructureContextMenu;

pub(super) fn append_indexes_section(
    container: &gtk::Box,
    structure: &TableStructure,
    context: StructureContextMenu,
) {
    let section = section_box(&gettext("Indexes"), structure.indexes.len());
    let list = list_box(&gettext("No indexes"), structure.indexes.is_empty());
    for index in &structure.indexes {
        let row = index_row(index);
        let mut table = structure.object.clone();
        table.schema.clone_from(&index.schema);
        context.attach(
            &row,
            StructureActionTarget::new(
                table,
                StructureActionKind::Index,
                index.name.clone(),
                !index.is_constraint_backed,
            ),
        );
        list.append(&row);
    }

    section.append(&list);
    container.append(&section);
}

pub(super) fn append_constraints_section(
    container: &gtk::Box,
    structure: &TableStructure,
    context: StructureContextMenu,
) {
    let section = section_box(&gettext("Constraints"), structure.constraints.len());

    let list = list_box(&gettext("No constraints"), structure.constraints.is_empty());
    for constraint in &structure.constraints {
        let row = constraint_row(constraint);
        attach_table_target(
            &context,
            &row,
            &structure.object,
            StructureActionKind::Constraint,
            &constraint.name,
        );
        list.append(&row);
    }

    section.append(&list);
    container.append(&section);
}

pub(super) fn append_foreign_keys_section(
    container: &gtk::Box,
    structure: &TableStructure,
    context: StructureContextMenu,
) {
    let section = section_box(&gettext("Foreign Keys"), structure.foreign_keys.len());

    let list = list_box(
        &gettext("No foreign keys"),
        structure.foreign_keys.is_empty(),
    );
    for foreign_key in &structure.foreign_keys {
        let row = foreign_key_row(foreign_key);
        attach_table_target(
            &context,
            &row,
            &structure.object,
            StructureActionKind::ForeignKey,
            &foreign_key.name,
        );
        list.append(&row);
    }

    section.append(&list);
    container.append(&section);
}

pub(super) fn append_triggers_section(
    container: &gtk::Box,
    structure: &TableStructure,
    context: StructureContextMenu,
) {
    let section = section_box(&gettext("Triggers"), structure.triggers.len());
    let list = list_box(&gettext("No triggers"), structure.triggers.is_empty());
    for trigger in &structure.triggers {
        let row = trigger_row(trigger);
        attach_table_target(
            &context,
            &row,
            &structure.object,
            StructureActionKind::Trigger,
            &trigger.name,
        );
        list.append(&row);
    }

    section.append(&list);
    container.append(&section);
}

pub(super) fn section_box(title: &str, count: usize) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    container.append(&section_label(&section_title(title, count)));
    container
}

pub(super) fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn list_box(empty_title: &str, is_empty: bool) -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    if is_empty {
        let row = adw::ActionRow::builder()
            .title(empty_title)
            .activatable(false)
            .build();
        row.add_css_class("dim-label");
        list.append(&row);
    }

    list
}

fn index_row(index: &TableIndex) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(escaped_markup(&index.name))
        .subtitle(escaped_markup(&index.definition))
        .activatable(false)
        .build();
    row.add_suffix(&detail_label(&index_summary(index)));
    row
}

fn constraint_row(constraint: &TableConstraint) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(escaped_markup(&constraint.name))
        .subtitle(escaped_markup(&constraint.definition))
        .activatable(false)
        .build();
    row.add_suffix(&detail_label(&constraint_summary(constraint)));
    row
}

fn foreign_key_row(foreign_key: &TableForeignKey) -> adw::ActionRow {
    let target = format!(
        "{}.{} ({})",
        foreign_key.referenced_schema,
        foreign_key.referenced_table,
        foreign_key.referenced_columns.join(", ")
    );
    let subtitle = format!(
        "{} -> {} · ON UPDATE {} · ON DELETE {}",
        foreign_key.columns.join(", "),
        target,
        foreign_key.on_update.label(),
        foreign_key.on_delete.label()
    );

    let row = adw::ActionRow::builder()
        .title(escaped_markup(&foreign_key.name))
        .subtitle(escaped_markup(&subtitle))
        .activatable(false)
        .build();
    add_detail_suffix(
        &row,
        &deferrable_summary(foreign_key.is_deferrable, foreign_key.is_initially_deferred),
    );
    row
}

fn trigger_row(trigger: &TableTrigger) -> adw::ActionRow {
    let subtitle = format!(
        "{}.{} · {}",
        trigger.function_schema, trigger.function_name, trigger.definition
    );

    let row = adw::ActionRow::builder()
        .title(escaped_markup(&trigger.name))
        .subtitle(escaped_markup(&subtitle))
        .activatable(false)
        .build();
    row.add_suffix(&detail_label(trigger.enabled.label()));
    row
}

fn attach_table_target(
    context: &StructureContextMenu,
    row: &adw::ActionRow,
    table: &DatabaseObject,
    kind: StructureActionKind,
    name: &str,
) {
    context.attach(
        row,
        StructureActionTarget::new(table.clone(), kind, name.to_string(), true),
    );
}

fn section_label(title: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(title));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("heading");
    label
}

fn add_detail_suffix(row: &adw::ActionRow, text: &str) {
    if !text.is_empty() {
        row.add_suffix(&detail_label(text));
    }
}

fn section_title(title: &str, count: usize) -> String {
    format!("{title} ({count})")
}

fn detail_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::End);
    label.set_valign(gtk::Align::Center);
    label.add_css_class("caption");
    label.add_css_class("dim-label");
    label
}

fn index_summary(index: &TableIndex) -> String {
    let mut parts = vec![index.method.clone()];

    if index.is_primary {
        parts.push(gettext("Primary"));
    } else if index.is_unique {
        parts.push(gettext("Unique"));
    }

    if !index.is_valid {
        parts.push(gettext("Invalid"));
    }

    if index.predicate.is_some() {
        parts.push(gettext("Partial"));
    }

    parts.join(" · ")
}

fn constraint_summary(constraint: &TableConstraint) -> String {
    let mut parts = vec![constraint.kind.label().to_string()];

    if !constraint.is_validated {
        parts.push(gettext("Not validated"));
    }

    let deferrable = deferrable_summary(constraint.is_deferrable, constraint.is_initially_deferred);
    if !deferrable.is_empty() {
        parts.push(deferrable);
    }

    parts.join(" · ")
}

fn deferrable_summary(is_deferrable: bool, is_initially_deferred: bool) -> String {
    match (is_deferrable, is_initially_deferred) {
        (true, true) => gettext("Deferred"),
        (true, false) => gettext("Deferrable"),
        (false, _) => String::new(),
    }
}

fn escaped_markup(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::escaped_markup;

    #[test]
    fn escaped_markup_handles_sql_operators() {
        assert_eq!(
            escaped_markup("attrs ->> 'e164' <> ''"),
            "attrs -&gt;&gt; &apos;e164&apos; &lt;&gt; &apos;&apos;"
        );
    }
}
