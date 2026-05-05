mod config;
mod db;
mod menus;
mod models;
mod settings;
mod state;
mod ui;
mod window;
mod window_content;

use config::{APP_ID, GETTEXT_PACKAGE, LOCALEDIR, PKGDATADIR};
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, setlocale, textdomain};
use relm4::RelmApp;
use relm4::gtk::gio;
use std::path::Path;
use window::AppWindow;

include!(concat!(env!("OUT_DIR"), "/icons"));

fn main() {
    setlocale(LocaleCategory::LcAll, "");
    setup_gettext();
    register_app_resources();

    relm4_icons::initialize_icons(GRESOURCE_BYTES, RESOURCE_PREFIX);

    if let Some(display) = relm4::gtk::gdk::Display::default() {
        let icon_theme = relm4::gtk::IconTheme::for_display(&display);

        if icon_theme.theme_name() == "hicolor" || icon_theme.theme_name().is_empty() {
            icon_theme.set_theme_name(Some("Adwaita"));
        }
    }

    let app = RelmApp::new(APP_ID);
    ui::style::load();
    app.run::<AppWindow>(());
}

fn setup_gettext() {
    bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("gettext textdomain to bind");
    bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8").expect("gettext codeset to bind");
    textdomain(GETTEXT_PACKAGE).expect("gettext textdomain to activate");
}

fn register_app_resources() {
    let resource_path = Path::new(PKGDATADIR).join("codd.gresource");
    let Ok(resources) = gio::Resource::load(resource_path) else {
        return;
    };

    gio::resources_register(&resources);
}
