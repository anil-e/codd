mod app_window;
mod config;
mod db;
mod menus;
mod models;
mod settings;
mod sql_file;
mod state;
mod ui;
mod window;
mod window_actions;
mod window_content;

use config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, PKGDATADIR};
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};
use relm4::RelmApp;
use relm4::gtk::gio;
use relm4::gtk::prelude::*;
use std::path::Path;
use window::AppWindow;

fn main() {
    setlocale(LocaleCategory::LcAll, "");
    setup_gettext();
    register_app_resources();
    setup_icon_theme();

    let app = RelmApp::new(APP_ID);
    let application = relm4::main_adw_application();
    setup_file_open(&application);
    ui::style::load();
    app.run::<AppWindow>(());
}

fn setup_file_open(application: &libadwaita::Application) {
    application.set_flags(application.flags() | gio::ApplicationFlags::HANDLES_OPEN);
    application.connect_open(|_, files, _| window::open_sql_files(files.to_vec()));
}

fn setup_gettext() {
    bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("gettext textdomain to bind");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8").expect("gettext codeset to bind");
    textdomain(GETTEXT_PACKAGE).expect("gettext textdomain to activate");
}

fn register_app_resources() {
    gio::resources_register_include!("codd.gresource").expect("bundled resources to register");

    let resource_path = Path::new(PKGDATADIR).join("codd.gresource");
    let Ok(resources) = gio::Resource::load(resource_path) else {
        return;
    };

    gio::resources_register(&resources);
}

fn setup_icon_theme() {
    relm4::main_adw_application().connect_startup(|_| {
        if let Some(display) = relm4::gtk::gdk::Display::default() {
            let icon_theme = relm4::gtk::IconTheme::for_display(&display);
            icon_theme.add_resource_path("/io/github/anil_e/codd/icons");

            if icon_theme.theme_name() == "hicolor" || icon_theme.theme_name().is_empty() {
                icon_theme.set_theme_name(Some("Adwaita"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    #[test]
    #[ignore = "requires a display and isolated XDG directories"]
    fn gtk_file_open_startup() {
        let app = RelmApp::<window::AppWindowMsg>::new(APP_ID);
        register_app_resources();
        setup_icon_theme();
        let application = relm4::main_adw_application();
        setup_file_open(&application);
        assert!(
            !application
                .flags()
                .contains(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        );
        let opened = Rc::new(Cell::new(false));
        application.connect_open({
            let opened = opened.clone();
            move |_, files, _| {
                assert_eq!(files.len(), 1);
                opened.set(true);
            }
        });
        let visible = Rc::new(Cell::new(false));
        application.connect_startup({
            let visible = visible.clone();
            move |application| {
                let application = application.clone();
                let visible = visible.clone();
                relm4::gtk::glib::timeout_add_local_once(Duration::from_millis(300), move || {
                    visible.set(
                        application
                            .active_window()
                            .is_some_and(|window| window.is_visible()),
                    );
                    application.quit();
                });
            }
        });
        app.with_args(vec!["codd".into(), "/tmp/codd-open-startup.sql".into()])
            .run::<AppWindow>(());
        assert!(
            opened.get(),
            "Relm4 must dispatch file arguments through GApplication::open"
        );
        assert!(
            visible.get(),
            "file-open startup must present a window without activate"
        );
    }
}
