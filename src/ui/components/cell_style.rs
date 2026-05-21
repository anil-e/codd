use libadwaita::prelude::*;
use relm4::gtk;

use crate::models::table_browser::ColumnTypeGroup;

const TYPE_CLASSES: &[&str] = &[
    "cell-type-boolean",
    "cell-type-binary",
    "cell-type-datetime",
    "cell-type-json",
    "cell-type-numeric",
    "cell-type-text",
];

pub fn apply_type_class(label: &gtk::Label, type_group: ColumnTypeGroup, is_dark: bool) {
    clear_type_classes(label);

    if let Some(class) = type_group_class(type_group) {
        label.add_css_class(class);
    }

    if is_dark {
        label.add_css_class("cell-dark");
    }
}

pub fn clear_type_classes(label: &gtk::Label) {
    for class in TYPE_CLASSES {
        label.remove_css_class(class);
    }

    label.remove_css_class("cell-dark");
}

pub fn type_group_class(type_group: ColumnTypeGroup) -> Option<&'static str> {
    match type_group {
        ColumnTypeGroup::Boolean => Some("cell-type-boolean"),
        ColumnTypeGroup::Binary => Some("cell-type-binary"),
        ColumnTypeGroup::DateTime => Some("cell-type-datetime"),
        ColumnTypeGroup::Json => Some("cell-type-json"),
        ColumnTypeGroup::Numeric => Some("cell-type-numeric"),
        ColumnTypeGroup::Text => Some("cell-type-text"),
        ColumnTypeGroup::Other => None,
    }
}
