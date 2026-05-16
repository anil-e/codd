use crate::db;
use crate::models::connection::{ConnectionDetails, ConnectionForm, SavedConnection};
use crate::state::credential_store;
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;
use sqlx::PgPool;

pub struct ConnectionDialogInit {
    pub parent_window: gtk::Window,
    pub connection: Option<SavedConnection>,
}

pub struct ConnectionDialog {
    form: ConnectionForm,
    is_busy: bool,
    saved_password_state: SavedPasswordState,
    credential_state: CredentialState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SavedPasswordState {
    Unknown,
    Available,
    Missing,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CredentialState {
    Checking,
    Available,
    Unavailable(String),
}

#[derive(Debug)]
pub enum ConnectionDialogMsg {
    NameChanged(String),
    HostChanged(String),
    PortChanged(String),
    DatabaseChanged(String),
    UsernameChanged(String),
    PasswordChanged(String),
    SavePasswordChanged(bool),
    TestConnection,
    Connect,
}

#[derive(Debug)]
pub enum ConnectionDialogOutput {
    Connected {
        details: ConnectionDetails,
        pool: PgPool,
    },
    Dismissed,
}

#[derive(Debug)]
pub enum ConnectionDialogCommandOutput {
    CredentialChecked(Result<(), String>),
    SavedPasswordChecked(Result<bool, String>),
    TestFinished(Result<(), String>),
    ConnectFinished(Result<(ConnectionDetails, PgPool), String>),
}

#[relm4::component(pub)]
impl Component for ConnectionDialog {
    type Init = ConnectionDialogInit;
    type Input = ConnectionDialogMsg;
    type Output = ConnectionDialogOutput;
    type CommandOutput = ConnectionDialogCommandOutput;

    view! {
        dialog = adw::Window {
            set_title: Some(&gettext("PostgreSQL Connection")),
            set_default_size: (460, 520),
            set_modal: true,

            connect_close_request[sender] => move |_| {
                let _ = sender.output(ConnectionDialogOutput::Dismissed);
                glib::Propagation::Proceed
            },

            #[wrap(Some)]
            #[name = "toast_overlay"]
            set_content = &adw::ToastOverlay {
                #[wrap(Some)]
                set_child = &adw::ToolbarView {
                    add_top_bar = &adw::HeaderBar {
                        set_title_widget: Some(&gtk::Label::new(Some(&gettext("PostgreSQL Connection")))),
                    },

                    #[wrap(Some)]
                    set_content = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 16,
                        set_margin_top: 18,
                        set_margin_bottom: 18,
                        set_margin_start: 18,
                        set_margin_end: 18,

                        adw::PreferencesGroup {
                            set_title: &gettext("Connection"),

                            adw::EntryRow {
                                set_title: &gettext("Name"),
                                set_text: &model.form.name,
                                #[watch]
                                set_sensitive: !model.is_busy,
                                connect_changed[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::NameChanged(row.text().to_string()));
                                },
                            },

                            adw::EntryRow {
                                set_title: &gettext("Host"),
                                set_text: &model.form.host,
                                #[watch]
                                set_sensitive: !model.is_busy,
                                connect_changed[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::HostChanged(row.text().to_string()));
                                },
                            },

                            adw::EntryRow {
                                set_title: &gettext("Port"),
                                set_text: &model.form.port,
                                set_input_purpose: gtk::InputPurpose::Digits,
                                #[watch]
                                set_sensitive: !model.is_busy,
                                connect_changed[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::PortChanged(row.text().to_string()));
                                },
                            },

                            adw::EntryRow {
                                set_title: &gettext("Default Database"),
                                set_text: &model.form.database,
                                #[watch]
                                set_sensitive: !model.is_busy,
                                connect_changed[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::DatabaseChanged(row.text().to_string()));
                                },
                            },

                            adw::EntryRow {
                                set_title: &gettext("Username"),
                                set_text: &model.form.username,
                                #[watch]
                                set_sensitive: !model.is_busy,
                                connect_changed[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::UsernameChanged(row.text().to_string()));
                                },
                            },

                            adw::PasswordEntryRow {
                                set_title: &gettext("Password"),
                                set_text: &model.form.password,
                                #[watch]
                                set_sensitive: !model.is_busy,
                                connect_changed[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::PasswordChanged(row.text().to_string()));
                                },
                            },

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 6,
                                set_margin_start: 16,
                                set_margin_end: 16,
                                set_margin_top: 6,
                                set_margin_bottom: 6,
                                #[watch]
                                set_visible: model.shows_password_status(),

                                gtk::Image {
                                    set_icon_name: Some("object-select-symbolic"),
                                    add_css_class: "success",
                                },

                                gtk::Label {
                                    #[watch]
                                    set_label: &model.password_status_text(),
                                    set_xalign: 0.0,
                                    set_wrap: true,
                                    add_css_class: "caption",
                                    add_css_class: "dim-label",
                                },
                            },

                            adw::SwitchRow {
                                set_title: &gettext("Save Password"),
                                #[watch]
                                set_subtitle: &model.save_password_subtitle(),
                                #[watch]
                                set_active: model.form.save_password,
                                #[watch]
                                set_sensitive: !model.is_busy && model.can_save_password(),
                                connect_active_notify[sender] => move |row| {
                                    sender.input(ConnectionDialogMsg::SavePasswordChanged(row.is_active()));
                                },
                            },
                        },

                        gtk::Box {
                            set_orientation: gtk::Orientation::Horizontal,
                            set_spacing: 12,
                            set_halign: gtk::Align::End,

                            gtk::Spinner {
                                #[watch]
                                set_visible: model.is_busy,
                                #[watch]
                                set_spinning: model.is_busy,
                            },

                            gtk::Button {
                                set_label: &gettext("Test Connection"),
                                #[watch]
                                set_sensitive: !model.is_busy && model.can_submit(),
                                connect_clicked => ConnectionDialogMsg::TestConnection,
                            },

                            gtk::Button {
                                set_label: &gettext("Connect"),
                                add_css_class: "suggested-action",
                                #[watch]
                                set_sensitive: !model.is_busy && model.can_submit(),
                                connect_clicked => ConnectionDialogMsg::Connect,
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        root.set_transient_for(Some(&init.parent_window));

        let model = ConnectionDialog {
            form: init
                .connection
                .as_ref()
                .map(ConnectionForm::from_saved)
                .unwrap_or_default(),
            is_busy: false,
            saved_password_state: SavedPasswordState::Unknown,
            credential_state: CredentialState::Checking,
        };
        let widgets = view_output!();

        sender.oneshot_command(async {
            ConnectionDialogCommandOutput::CredentialChecked(
                credential_store::is_available()
                    .await
                    .map_err(|error| error.to_string()),
            )
        });

        if let Some(connection) = init
            .connection
            .filter(|connection| connection.save_password)
        {
            sender.oneshot_command(async move {
                ConnectionDialogCommandOutput::SavedPasswordChecked(
                    credential_store::has_password(&connection.id)
                        .await
                        .map_err(|error| error.to_string()),
                )
            });
        }

        root.present();

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            ConnectionDialogMsg::NameChanged(value) => self.form.name = value,
            ConnectionDialogMsg::HostChanged(value) => self.form.host = value,
            ConnectionDialogMsg::PortChanged(value) => self.form.port = value,
            ConnectionDialogMsg::DatabaseChanged(value) => self.form.database = value,
            ConnectionDialogMsg::UsernameChanged(value) => self.form.username = value,
            ConnectionDialogMsg::PasswordChanged(value) => self.form.password = value,
            ConnectionDialogMsg::SavePasswordChanged(value) => {
                self.form.save_password = value && self.can_save_password();
            }

            ConnectionDialogMsg::TestConnection => {
                let Some(details) = self.validated_details(widgets) else {
                    return;
                };

                self.is_busy = true;
                sender.oneshot_command(async move {
                    ConnectionDialogCommandOutput::TestFinished(test_connection(details).await)
                });
            }

            ConnectionDialogMsg::Connect => {
                let Some(details) = self.validated_details(widgets) else {
                    return;
                };

                self.is_busy = true;
                sender.oneshot_command(async move {
                    ConnectionDialogCommandOutput::ConnectFinished(connect(details).await)
                });
            }
        }

        self.update_view(widgets, sender);
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        self.is_busy = false;

        match msg {
            ConnectionDialogCommandOutput::CredentialChecked(Ok(())) => {
                self.credential_state = CredentialState::Available;
            }

            ConnectionDialogCommandOutput::CredentialChecked(Err(error)) => {
                self.credential_state = CredentialState::Unavailable(error);
                self.form.save_password = false;
            }

            ConnectionDialogCommandOutput::SavedPasswordChecked(Ok(true)) => {
                self.saved_password_state = SavedPasswordState::Available;
            }

            ConnectionDialogCommandOutput::SavedPasswordChecked(Ok(false)) => {
                self.saved_password_state = SavedPasswordState::Missing;
            }

            ConnectionDialogCommandOutput::SavedPasswordChecked(Err(error)) => {
                self.saved_password_state = SavedPasswordState::Error;
                show_error_dialog(root, &gettext("Reading the saved password failed"), &error);
            }

            ConnectionDialogCommandOutput::TestFinished(Ok(())) => {
                if let Some(toast_overlay) = root.toast_overlay() {
                    toast_overlay
                        .add_toast(adw::Toast::new(&gettext("Connection test succeeded.")));
                }
            }

            ConnectionDialogCommandOutput::TestFinished(Err(error)) => {
                show_error_dialog(root, &gettext("Connection test failed"), &error);
            }

            ConnectionDialogCommandOutput::ConnectFinished(Ok((details, pool))) => {
                let _ = sender.output(ConnectionDialogOutput::Connected { details, pool });
                root.close();
            }

            ConnectionDialogCommandOutput::ConnectFinished(Err(error)) => {
                show_error_dialog(root, &gettext("Connection failed"), &error);
            }
        }
    }
}

impl ConnectionDialog {
    fn can_submit(&self) -> bool {
        !self.form.name.trim().is_empty()
            && !self.form.host.trim().is_empty()
            && self.form.port.trim().parse::<u16>().is_ok()
            && !self.form.database.trim().is_empty()
            && !self.form.username.trim().is_empty()
    }

    fn validated_details(&self, widgets: &ConnectionDialogWidgets) -> Option<ConnectionDetails> {
        match self.form.validate() {
            Ok(details) => Some(details),
            Err(error) => {
                widgets.toast_overlay.add_toast(adw::Toast::new(&error));
                None
            }
        }
    }

    fn save_password_subtitle(&self) -> String {
        match &self.credential_state {
            CredentialState::Checking => gettext("Checking password storage availability."),
            CredentialState::Available => gettext("Store this password in GNOME Keyring."),
            CredentialState::Unavailable(_) => gettext("Password storage is not available."),
        }
    }

    fn shows_password_status(&self) -> bool {
        self.form.save_password
            && self.form.password.is_empty()
            && self.form.id.is_some()
            && matches!(
                self.saved_password_state,
                SavedPasswordState::Available | SavedPasswordState::Missing
            )
    }

    fn password_status_text(&self) -> String {
        match self.saved_password_state {
            SavedPasswordState::Available => {
                gettext("Saved in GNOME Keyring. Enter a new password to replace it.")
            }
            SavedPasswordState::Missing => {
                gettext("No saved password was found. Enter a password to save it.")
            }
            SavedPasswordState::Unknown | SavedPasswordState::Error => String::new(),
        }
    }

    fn can_save_password(&self) -> bool {
        self.credential_state == CredentialState::Available
    }
}

trait WindowToastOverlay {
    fn toast_overlay(&self) -> Option<adw::ToastOverlay>;
}

impl WindowToastOverlay for adw::Window {
    fn toast_overlay(&self) -> Option<adw::ToastOverlay> {
        self.content().and_downcast::<adw::ToastOverlay>()
    }
}

async fn test_connection(details: ConnectionDetails) -> Result<(), String> {
    let details = details_with_saved_password(details).await?;
    db::postgres::test_connection(&details)
        .await
        .map_err(|error| error.to_string())
}

async fn connect(details: ConnectionDetails) -> Result<(ConnectionDetails, PgPool), String> {
    let details = details_with_saved_password(details).await?;
    let pool = db::postgres::connect(&details)
        .await
        .map_err(|error| error.to_string())?;

    Ok((details, pool))
}

async fn details_with_saved_password(
    mut details: ConnectionDetails,
) -> Result<ConnectionDetails, String> {
    if !details.password.is_empty() || !details.saved.save_password {
        return Ok(details);
    }

    if let Some(password) = credential_store::load_password(&details.saved.id)
        .await
        .map_err(|error| format!("{}: {error}", gettext("Reading the saved password failed")))?
    {
        details.password = password;
    }

    Ok(details)
}

fn show_error_dialog(parent: &adw::Window, heading: &str, error: &str) {
    let label = gtk::Label::builder()
        .label(error)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .build();

    let scrolled = gtk::ScrolledWindow::builder()
        .min_content_height(96)
        .max_content_height(240)
        .propagate_natural_height(true)
        .child(&label)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(gettext("PostgreSQL returned the following error:"))
        .extra_child(&scrolled)
        .close_response("close")
        .build();

    dialog.add_response("close", &gettext("Close"));
    dialog.present(Some(parent));
}
