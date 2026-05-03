use crate::config::{
    APP_ID, APP_SETTINGS_PATH, CONNECTION_STATE_PATH_PREFIX, CONNECTION_STATE_SCHEMA_ID,
};
use relm4::gtk::gio;

pub fn app_settings() -> gio::Settings {
    load_settings(APP_ID, Some(APP_SETTINGS_PATH))
}

pub fn connection_state_settings(connection_id: &str) -> gio::Settings {
    let path = format!("{CONNECTION_STATE_PATH_PREFIX}{connection_id}/");
    load_settings(CONNECTION_STATE_SCHEMA_ID, Some(&path))
}

fn load_settings(schema_id: &str, path: Option<&str>) -> gio::Settings {
    let default_source = gio::SettingsSchemaSource::default();
    let source = gio::SettingsSchemaSource::from_directory(
        concat!(env!("OUT_DIR"), "/schemas"),
        default_source.as_ref(),
        false,
    )
    .ok()
    .or(default_source);

    let schema = source
        .as_ref()
        .and_then(|source| source.lookup(schema_id, true))
        .unwrap_or_else(|| panic!("missing GSettings schema: {schema_id}"));

    gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, path)
}
