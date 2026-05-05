use gettextrs::gettext;
use relm4::gtk;

pub(crate) fn main_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();

    let connection_section = gtk::gio::Menu::new();
    connection_section.append(
        Some(&gettext("_New Connection")),
        Some("win.new-connection"),
    );
    menu.append_section(None, &connection_section);

    let app_section = gtk::gio::Menu::new();
    app_section.append(Some(&gettext("_Keyboard Shortcuts")), Some("app.shortcuts"));
    app_section.append(Some(&gettext("_Quit")), Some("app.quit"));
    menu.append_section(None, &app_section);

    menu
}
