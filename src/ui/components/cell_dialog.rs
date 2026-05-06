use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;

pub fn show_cell_value_dialog(anchor: &gtk::Label, value: &str) {
    let parent = anchor.root().and_downcast::<gtk::Window>();

    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(value);

    let text = gtk::TextView::builder()
        .buffer(&buffer)
        .editable(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(16)
        .bottom_margin(16)
        .left_margin(16)
        .right_margin(16)
        .build();

    let scrolled = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&text)
        .build();

    let dialog = adw::Window::builder()
        .title(gettext("Cell value"))
        .default_width(720)
        .default_height(520)
        .modal(true)
        .build();

    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::new(Some(&gettext("Cell value")))));
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));
    dialog.set_content(Some(&toolbar));

    dialog.present();
}
