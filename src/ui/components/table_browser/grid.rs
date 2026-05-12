use std::borrow::Cow;

use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::models::table_browser::{ColumnTypeGroup, TableCell};
use crate::ui::components::cell_dialog::show_cell_value_dialog;
use crate::ui::components::table_browser::{TableBrowser, TableBrowserMsg};

const DISPLAY_CHAR_LIMIT: usize = 256;
const TOOLTIP_CHAR_LIMIT: usize = 2048;

fn truncated(value: &str, limit: usize) -> Cow<'_, str> {
    match value.char_indices().nth(limit) {
        Some((end, _)) => {
            let mut out = String::with_capacity(end + '…'.len_utf8());
            out.push_str(&value[..end]);
            out.push('…');

            Cow::Owned(out)
        }

        None => Cow::Borrowed(value),
    }
}

#[derive(Debug, Clone)]
pub(super) struct TableBrowserRow {
    pub(super) index: usize,
    pub(super) cells: Vec<TableCell>,
}

pub(super) fn clear_columns(view: &gtk::ColumnView) {
    while let Some(column) = view.columns().item(0) {
        if let Ok(column) = column.downcast::<gtk::ColumnViewColumn>() {
            view.remove_column(&column);
        } else {
            break;
        }
    }
}

pub(super) fn cell_factory(
    column_index: usize,
    type_group: ColumnTypeGroup,
    is_dark: bool,
    sender: &ComponentSender<TableBrowser>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let color = type_group_color(type_group, is_dark);
    let sender = sender.clone();

    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let list_item = list_item.clone();
        let sender = sender.clone();

        let label = gtk::Label::builder()
            .xalign(0.0)
            .selectable(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .lines(1)
            .width_chars(12)
            .max_width_chars(28)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();

        label.add_css_class("query-cell");
        label.set_use_markup(color.is_some());

        label.add_controller({
            let gesture = gtk::GestureClick::new();
            let list_item = list_item.clone();
            gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

            gesture.connect_pressed(move |gesture, press_count, _, _| {
                if press_count == 2
                    && let Some(widget) = gesture.widget()
                    && let Ok(label) = widget.downcast::<gtk::Label>()
                    && let Some(item) = list_item.item()
                    && let Ok(row) = item.downcast::<glib::BoxedAnyObject>()
                {
                    let row = row.borrow::<TableBrowserRow>();

                    if gesture.current_button() == gtk::gdk::BUTTON_PRIMARY {
                        sender.input(TableBrowserMsg::EditCellRequested {
                            anchor: label.clone(),
                            row_index: row.index,
                            column_index,
                        });
                    } else if let Some(cell) = row.cells.get(column_index) {
                        show_cell_value_dialog(&label, &cell.value);
                    }
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

        let row = row.borrow::<TableBrowserRow>();
        let value = row
            .cells
            .get(column_index)
            .map_or("", |cell| cell.value.as_str());

        let display = truncated(value, DISPLAY_CHAR_LIMIT);

        if let Some(color) = color {
            let escaped_value = glib::markup_escape_text(display.as_ref());
            label.set_markup(&format!(
                "<span foreground=\"{color}\">{escaped_value}</span>"
            ));
        } else {
            label.set_label(display.as_ref());
        }

        label.set_tooltip_text(Some(truncated(value, TOOLTIP_CHAR_LIMIT).as_ref()));
    });

    factory
}

fn type_group_color(type_group: ColumnTypeGroup, is_dark: bool) -> Option<&'static str> {
    match (type_group, is_dark) {
        (ColumnTypeGroup::Boolean, false) => Some("#1f9d55"),
        (ColumnTypeGroup::Boolean, true) => Some("#4fd785"),
        (ColumnTypeGroup::Binary, false) => Some("#8b5cf6"),
        (ColumnTypeGroup::Binary, true) => Some("#b89cff"),
        (ColumnTypeGroup::DateTime, false) => Some("#0f7abf"),
        (ColumnTypeGroup::DateTime, true) => Some("#66c2ff"),
        (ColumnTypeGroup::Json, false) => Some("#d97706"),
        (ColumnTypeGroup::Json, true) => Some("#ffb15f"),
        (ColumnTypeGroup::Numeric, false) => Some("#6d28d9"),
        (ColumnTypeGroup::Numeric, true) => Some("#c69cff"),
        (ColumnTypeGroup::Text, false) => Some("#0057b7"),
        (ColumnTypeGroup::Text, true) => Some("#79b8ff"),
        (ColumnTypeGroup::Other, _) => None,
    }
}
