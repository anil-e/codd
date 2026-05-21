use std::borrow::Cow;

use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::models::table_browser::{ColumnTypeGroup, TableCell, TableColumn};
use crate::ui::components::cell_dialog::show_cell_value_dialog;
use crate::ui::components::cell_style;
use crate::ui::components::table_browser::{TableBrowser, TableBrowserMsg};

use super::sorting::sync_sort_indicator;

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
    let type_class = cell_style::type_group_class(type_group);
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
        if let Some(class) = type_class {
            label.add_css_class(class);

            if is_dark {
                label.add_css_class("cell-dark");
            }
        }

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

        label.set_label(truncated(value, DISPLAY_CHAR_LIMIT).as_ref());
        label.set_tooltip_text(Some(truncated(value, TOOLTIP_CHAR_LIMIT).as_ref()));
    });

    factory
}

pub(super) fn render_table(
    table_browser: &mut TableBrowser,
    sender: &ComponentSender<TableBrowser>,
) {
    let Some(page) = table_browser.page.clone() else {
        table_browser.table_rows.remove_all();
        return;
    };

    sync_columns(table_browser, &page.columns, sender);
    table_browser.table_rows.remove_all();

    for (index, row) in page.rows.iter().enumerate() {
        table_browser
            .table_rows
            .append(&glib::BoxedAnyObject::new(TableBrowserRow {
                index,
                cells: row.clone(),
            }));
    }
}

fn sync_columns(
    table_browser: &mut TableBrowser,
    columns: &[TableColumn],
    sender: &ComponentSender<TableBrowser>,
) {
    let column_keys = columns
        .iter()
        .map(|column| format!("{}\u{1f}{}", column.name, column.display_type))
        .collect::<Vec<_>>();

    if table_browser.rendered_columns == column_keys {
        return;
    }

    clear_columns(&table_browser.table_view);
    table_browser.rendered_columns = column_keys;

    let is_dark = table_browser.style_manager.is_dark();

    for (index, column) in columns.iter().enumerate() {
        let factory = cell_factory(index, column.type_group, is_dark, sender);
        let title = column.name.clone();
        let view_column = gtk::ColumnViewColumn::new(Some(&title), Some(factory));
        view_column.set_resizable(true);
        view_column.set_expand(index < 3);
        view_column.set_sorter(Some(&gtk::CustomSorter::new(|_, _| {
            std::cmp::Ordering::Equal.into()
        })));
        table_browser.table_view.append_column(&view_column);
    }

    sync_sort_indicator(&table_browser.table_view, table_browser.sort.as_ref());
}
