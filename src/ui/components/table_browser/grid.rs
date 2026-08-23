use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;

use crate::models::database_object::DatabaseObjectKind;
use crate::models::table_browser::{ColumnTypeGroup, TableCell, TableColumn};
use crate::ui::components::cell_dialog::show_cell_value_dialog;
use crate::ui::components::cell_style;
use crate::ui::components::table_browser::{CopyTarget, EditTarget, TableBrowser, TableBrowserMsg};

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

#[derive(Clone)]
struct CellFactoryContext {
    sender: ComponentSender<TableBrowser>,
    context_popover: gtk::PopoverMenu,
    copy_target: Rc<Cell<Option<CopyTarget>>>,
    edit_target: Rc<RefCell<Option<EditTarget>>>,
    edit_action: gio::SimpleAction,
    duplicate_action: gio::SimpleAction,
    delete_action: gio::SimpleAction,
    selection: gtk::SingleSelection,
    busy: Rc<Cell<bool>>,
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

fn cell_factory(
    column_index: usize,
    type_group: ColumnTypeGroup,
    is_dark: bool,
    can_edit: bool,
    can_duplicate: bool,
    can_delete: bool,
    context: CellFactoryContext,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let type_class = cell_style::type_group_class(type_group);

    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let list_item = list_item.clone();
        let context = context.clone();

        let label = gtk::Label::builder()
            .xalign(0.0)
            .focusable(false)
            .selectable(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .lines(1)
            .width_chars(12)
            .max_width_chars(28)
            .build();

        label.add_css_class("query-cell");
        label.add_css_class("data-cell");
        if let Some(class) = type_class {
            label.add_css_class(class);

            if is_dark {
                label.add_css_class("cell-dark");
            }
        }

        label.add_controller({
            let gesture = gtk::GestureClick::new();
            let list_item = list_item.clone();
            let context = context.clone();

            gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
            gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            gesture.connect_pressed(move |gesture, _, x, y| {
                let Some(widget) = gesture.widget() else {
                    return;
                };
                let Ok(label) = widget.downcast::<gtk::Label>() else {
                    return;
                };

                gesture.set_state(gtk::EventSequenceState::Claimed);

                if let Some(item) = list_item.item()
                    && let Ok(row) = item.downcast::<glib::BoxedAnyObject>()
                {
                    let row = row.borrow::<TableBrowserRow>();
                    context.copy_target.set(Some(CopyTarget {
                        row_index: row.index,
                        column_index,
                    }));
                    *context.edit_target.borrow_mut() = Some(EditTarget {
                        anchor: label.clone(),
                        row_index: row.index,
                        column_index,
                    });
                    context.selection.set_selected(row.index as u32);
                    context
                        .edit_action
                        .set_enabled(!context.busy.get() && can_edit);
                    context
                        .duplicate_action
                        .set_enabled(!context.busy.get() && can_duplicate);
                    context
                        .delete_action
                        .set_enabled(!context.busy.get() && can_delete);
                    show_context_menu(&label, &context.context_popover, x, y);
                }
            });
            gesture
        });

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
                        context.sender.input(TableBrowserMsg::EditCellRequested {
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

fn show_context_menu(anchor: &gtk::Label, popover: &gtk::PopoverMenu, x: f64, y: f64) {
    if let Some(parent) = popover.parent()
        && let Some(point) =
            anchor.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
    {
        let rect = gtk::gdk::Rectangle::new(point.x() as i32, point.y() as i32, 1, 1);
        popover.set_pointing_to(Some(&rect));
    }

    popover.popup();
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
        .map(|column| {
            format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                column.name,
                column.display_type,
                column.type_name,
                column.enum_values.join("\u{1e}"),
                column.is_nullable,
                column.is_primary_key,
                column.has_default,
                column.is_identity,
                column.is_generated,
            )
        })
        .collect::<Vec<_>>();

    if table_browser.rendered_columns == column_keys {
        return;
    }

    clear_columns(&table_browser.table_view);
    table_browser.rendered_columns = column_keys;

    let is_dark = table_browser.style_manager.is_dark();
    let can_edit = table_browser.page.as_ref().is_some_and(|page| {
        page.object.kind == DatabaseObjectKind::Table
            && page.columns.iter().any(|column| column.is_primary_key)
    });
    let can_duplicate = table_browser
        .page
        .as_ref()
        .is_some_and(|page| page.object.kind == DatabaseObjectKind::Table);
    let context = CellFactoryContext {
        sender: sender.clone(),
        context_popover: table_browser.context_popover.clone(),
        copy_target: table_browser.copy_target.clone(),
        edit_target: table_browser.edit_target.clone(),
        edit_action: table_browser.edit_action.clone(),
        duplicate_action: table_browser.duplicate_action.clone(),
        delete_action: table_browser.delete_action.clone(),
        selection: table_browser.selection.clone(),
        busy: table_browser.context_busy.clone(),
    };
    let can_delete = table_browser.can_delete_rows();

    for (index, column) in columns.iter().enumerate() {
        let can_edit = can_edit && !column.is_primary_key && column.is_editable_value_type();
        let factory = cell_factory(
            index,
            column.type_group,
            is_dark,
            can_edit,
            can_duplicate,
            can_delete,
            context.clone(),
        );
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
