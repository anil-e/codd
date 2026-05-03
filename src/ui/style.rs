use crate::config::RESOURCE_PREFIX;
use relm4::gtk::{self, gio};

pub fn load() {
    let resource_path = format!("{RESOURCE_PREFIX}/style.css");
    if gio::resources_lookup_data(&resource_path, gio::ResourceLookupFlags::NONE).is_err() {
        relm4::set_global_css(include_str!("style.css"));
        return;
    }

    let Some(display) = gtk::gdk::Display::default() else {
        relm4::set_global_css(include_str!("style.css"));
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_resource(&resource_path);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
