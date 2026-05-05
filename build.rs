fn main() {
    println!("cargo:rerun-if-changed=data/io.github.anil_e.Codd.gschema.xml");
    println!("cargo:rerun-if-env-changed=CODD_APP_ID");
    println!("cargo:rerun-if-env-changed=CODD_APP_SETTINGS_PATH");
    println!("cargo:rerun-if-env-changed=CODD_CONNECTION_STATE_SCHEMA_ID");
    println!("cargo:rerun-if-env-changed=CODD_CONNECTION_STATE_PATH_PREFIX");
    println!("cargo:rerun-if-env-changed=CODD_GETTEXT_PACKAGE");
    println!("cargo:rerun-if-env-changed=CODD_LOCALEDIR");
    println!("cargo:rerun-if-env-changed=CODD_PKGDATADIR");

    relm4_icons_build::bundle_icons(
        "icons",
        None::<&str>,
        None::<&str>,
        None::<&str>,
        [
            "add",
            "database",
            "database-regular",
            "delete-regular",
            "document-edit-regular",
            "edit-regular",
            "error",
            "go-previous",
            "go-next",
            "media-playback-start",
            "network-server",
            "open-menu",
            "preview",
            "running",
            "sidebar-left",
            "table",
            "view-list",
        ],
    );

    write_config();
    compile_gsettings_schema();
}

fn write_config() {
    use std::fmt::Write as _;
    use std::path::PathBuf;

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR to be set"));
    let config_path = out_dir.join("config.rs");
    let mut config = String::new();

    for (name, value) in [
        ("APP_ID", env_or("CODD_APP_ID", "io.github.anil_e.Codd")),
        (
            "APP_SETTINGS_PATH",
            env_or("CODD_APP_SETTINGS_PATH", "/io/github/anil_e/codd/"),
        ),
        (
            "CONNECTION_STATE_SCHEMA_ID",
            env_or(
                "CODD_CONNECTION_STATE_SCHEMA_ID",
                "io.github.anil_e.Codd.connection-state",
            ),
        ),
        (
            "CONNECTION_STATE_PATH_PREFIX",
            env_or(
                "CODD_CONNECTION_STATE_PATH_PREFIX",
                "/io/github/anil_e/codd/connection-state/",
            ),
        ),
        (
            "PKGDATADIR",
            env_or("CODD_PKGDATADIR", "/usr/local/share/codd"),
        ),
        ("GETTEXT_PACKAGE", env_or("CODD_GETTEXT_PACKAGE", "codd")),
        (
            "LOCALEDIR",
            env_or("CODD_LOCALEDIR", "/usr/local/share/locale"),
        ),
        ("RESOURCE_PREFIX", "/io/github/anil_e/codd".to_string()),
    ] {
        let _ = writeln!(config, "pub const {name}: &str = {value:?};");
    }

    std::fs::write(config_path, config).expect("config.rs to be generated");
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn compile_gsettings_schema() {
    use std::path::PathBuf;
    use std::process::Command;

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR to be set"));
    let schema_dir = out_dir.join("schemas");
    std::fs::create_dir_all(&schema_dir).expect("schema output directory to be created");

    std::fs::copy(
        "data/io.github.anil_e.Codd.gschema.xml",
        schema_dir.join("io.github.anil_e.Codd.gschema.xml"),
    )
    .expect("GSettings schema to be copied");

    let status = Command::new("glib-compile-schemas")
        .arg(&schema_dir)
        .status()
        .expect("glib-compile-schemas to run");

    assert!(status.success(), "glib-compile-schemas failed");
}
