use gettextrs::gettext;
use relm4::gtk;

pub(crate) fn start_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();

    menu.append_section(None, &window_menu_section());

    let connection_section = gtk::gio::Menu::new();
    connection_section.append(
        Some(&gettext("_New Connection")),
        Some("win.new-connection"),
    );
    menu.append_section(None, &connection_section);

    menu.append_section(None, &app_menu_section());

    menu
}

pub(crate) fn workspace_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();

    menu.append_section(None, &window_menu_section());
    menu.append_section(None, &app_menu_section());

    menu
}

fn window_menu_section() -> gtk::gio::Menu {
    let section = gtk::gio::Menu::new();
    section.append(Some(&gettext("_New Window")), Some("app.new-window"));
    section
}

fn app_menu_section() -> gtk::gio::Menu {
    let section = gtk::gio::Menu::new();
    section.append(Some(&gettext("_Keyboard Shortcuts")), Some("app.shortcuts"));
    section.append(Some(&gettext("_About Codd")), Some("app.about"));
    section.append(Some(&gettext("_Quit")), Some("app.quit"));
    section
}
