use std::rc::Rc;

use gettextrs::gettext;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::models::table_browser::{ColumnTypeGroup, TableCell, TableColumn};
use crate::ui::components::table_browser::{TableBrowser, TableBrowserMsg};

pub(super) fn show_edit_cell_popover(
    anchor: &gtk::Label,
    column: &TableColumn,
    cell: &TableCell,
    sender: ComponentSender<TableBrowser>,
    page_generation: u64,
    row_index: usize,
    column_index: usize,
) -> gtk::Popover {
    let editor = CellEditor::new(column, cell);

    let column_label = gtk::Label::builder()
        .label(&column.name)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();

    column_label.add_css_class("heading");

    let type_label = gtk::Label::builder()
        .label(format!("{}: {}", gettext("Type"), column.display_type))
        .halign(gtk::Align::Start)
        .build();

    type_label.add_css_class("caption");
    type_label.add_css_class("dim-label");

    let value_label = gtk::Label::builder()
        .label(gettext("Value"))
        .halign(gtk::Align::Start)
        .build();

    value_label.add_css_class("caption");
    value_label.add_css_class("dim-label");

    let value_hint = gtk::Label::builder()
        .label(if column.is_nullable {
            gettext("Use Set NULL to store a database NULL. Empty text is saved as empty text.")
        } else {
            gettext("This column is not nullable.")
        })
        .halign(gtk::Align::Start)
        .wrap(true)
        .build();

    value_hint.add_css_class("caption");
    value_hint.add_css_class("dim-label");

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .width_request(360)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();

    header.append(&column_label);
    header.append(&type_label);

    let value_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();

    value_box.append(&value_label);
    value_box.append(&editor.widget);
    value_box.append(&value_hint);

    let actions = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();

    let cancel_button = gtk::Button::with_label(&gettext("Cancel"));
    let null_button = gtk::Button::with_label(&gettext("Set NULL"));

    null_button.add_css_class("flat");
    null_button.set_visible(column.is_nullable);

    let save_button = gtk::Button::with_label(&gettext("Save"));

    save_button.add_css_class("suggested-action");

    actions.append(&cancel_button);
    actions.append(&null_button);
    actions.append(&save_button);

    content.append(&header);
    content.append(&value_box);
    content.append(&actions);

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(gtk::PositionType::Bottom)
        .child(&content)
        .build();

    popover.set_parent(anchor);

    popover.connect_closed(|popover| {
        if popover.parent().is_some() {
            popover.unparent();
        }
    });

    cancel_button.connect_clicked({
        let popover = popover.clone();

        move |_| {
            popover.popdown();
        }
    });

    null_button.connect_clicked({
        let popover = popover.clone();
        let sender = sender.clone();

        move |_| {
            submit_cell_edit(
                &popover,
                &sender,
                page_generation,
                row_index,
                column_index,
                None,
            );
        }
    });

    save_button.connect_clicked({
        let popover = popover.clone();
        let editor_value = editor.value.clone();
        let sender = sender.clone();

        move |_| {
            submit_cell_edit(
                &popover,
                &sender,
                page_generation,
                row_index,
                column_index,
                Some(editor_value()),
            );
        }
    });

    popover.popup();
    editor.grab_focus();

    popover
}

fn submit_cell_edit(
    popover: &gtk::Popover,
    sender: &ComponentSender<TableBrowser>,
    page_generation: u64,
    row_index: usize,
    column_index: usize,
    submitted_value: Option<String>,
) {
    popover.set_sensitive(false);
    popover.popdown();

    let sender = sender.clone();

    glib::idle_add_local_once(move || {
        sender.input(TableBrowserMsg::CellEditSubmitted {
            page_generation,
            row_index,
            column_index,
            value: submitted_value,
        });
    });
}

struct CellEditor {
    widget: gtk::Widget,
    value: Rc<dyn Fn() -> String>,
    focus: gtk::Widget,
}

impl CellEditor {
    fn new(column: &TableColumn, cell: &TableCell) -> Self {
        if column.type_group == ColumnTypeGroup::Boolean {
            return Self::choice(
                vec!["FALSE".to_string(), "TRUE".to_string()],
                boolean_selection(&cell.value),
            );
        }

        if !column.enum_values.is_empty() {
            let selected = column
                .enum_values
                .iter()
                .position(|value| value == &cell.value)
                .unwrap_or(0);

            return Self::choice(column.enum_values.clone(), selected);
        }

        Self::text(cell)
    }

    fn text(cell: &TableCell) -> Self {
        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(if cell.is_null { "" } else { &cell.value });

        let value_editor = gtk::TextView::builder()
            .buffer(&buffer)
            .editable(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(10)
            .bottom_margin(10)
            .left_margin(10)
            .right_margin(10)
            .height_request(180)
            .build();

        let scrolled = gtk::ScrolledWindow::builder()
            .hexpand(true)
            .vexpand(true)
            .min_content_height(180)
            .child(&value_editor)
            .build();

        let frame = gtk::Frame::new(None);
        frame.set_child(Some(&scrolled));

        let value = Rc::new(move || {
            buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .to_string()
        });

        Self {
            widget: frame.upcast(),
            value,
            focus: value_editor.upcast(),
        }
    }

    fn choice(values: Vec<String>, selected: usize) -> Self {
        let model = string_list(&values);
        let dropdown = gtk::DropDown::builder()
            .model(&model)
            .selected(selected as u32)
            .build();
        dropdown.add_css_class("compact");

        let values = Rc::new(values);
        let value = {
            let dropdown = dropdown.clone();
            let values = values.clone();

            Rc::new(move || {
                values
                    .get(dropdown.selected() as usize)
                    .cloned()
                    .unwrap_or_default()
            })
        };

        Self {
            widget: dropdown.clone().upcast(),
            value,
            focus: dropdown.upcast(),
        }
    }

    fn grab_focus(&self) {
        self.focus.grab_focus();
    }
}

fn boolean_selection(value: &str) -> usize {
    if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("t") {
        1
    } else {
        0
    }
}

fn string_list(values: &[String]) -> gtk::StringList {
    let borrowed = values.iter().map(String::as_str).collect::<Vec<_>>();

    gtk::StringList::new(&borrowed)
}
