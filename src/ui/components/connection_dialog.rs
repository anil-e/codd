use crate::db;
use crate::db::postgres::PostgresConnection;
use crate::models::connection::{
    ConnectionDetails, ConnectionForm, SavedConnection, SshAuthMethod,
};
use crate::state::{connection_store, credential_store};
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;

pub struct ConnectionDialogInit {
    pub parent_window: gtk::Window,
    pub connection: Option<SavedConnection>,
}

pub struct ConnectionDialog {
    form: ConnectionForm,
    ssh_auth_method_model: gtk::StringList,
    is_busy: bool,
    saved_password_state: SavedPasswordState,
    credential_state: CredentialState,
    pending_trusted_host_key_save: bool,
    persist_trusted_host_key_after_test: bool,
}

#[derive(Debug)]
pub struct ConnectionDialogConnected {
    pub details: ConnectionDetails,
    pub connection: PostgresConnection,
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
    SshEnabledChanged(bool),
    SshHostChanged(String),
    SshPortChanged(String),
    SshUsernameChanged(String),
    SshAuthMethodSelected(u32),
    SshPasswordChanged(String),
    SelectSshPrivateKeyFile,
    SshPrivateKeyFileSelected(String),
    SshKeyPassphraseChanged(String),
    SshSaveSecretChanged(bool),
    TrustSshHostKeyAndConnect(String),
    TrustSshHostKeyAndTest(String),
    TestConnection,
    Connect,
}

#[derive(Debug)]
pub enum ConnectionDialogOutput {
    Connected(Box<ConnectionDialogConnected>),
    Dismissed,
}

#[derive(Debug)]
pub enum ConnectionDialogCommandOutput {
    CredentialChecked(Result<(), String>),
    SavedPasswordChecked(Result<bool, String>),
    TestFinished(Result<(), ConnectionDialogError>),
    ConnectFinished(Result<Box<ConnectionDialogConnected>, ConnectionDialogError>),
}

#[derive(Debug)]
pub enum ConnectionDialogError {
    Message(String),
    UntrustedSshHostKey(String),
}

impl std::fmt::Display for ConnectionDialogError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => write!(formatter, "{message}"),
            Self::UntrustedSshHostKey(fingerprint) => write!(
                formatter,
                "{} {fingerprint}",
                gettext("SSH host key is not trusted yet. Fingerprint:")
            ),
        }
    }
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
            set_default_size: (500, 680),
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
                    set_content = &gtk::ScrolledWindow {
                        set_propagate_natural_height: true,

                        #[wrap(Some)]
                        set_child = &gtk::Box {
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

                            adw::PreferencesGroup {
                                set_title: &gettext("SSH Tunnel"),

                                adw::SwitchRow {
                                    set_title: &gettext("Use SSH Tunnel"),
                                    #[watch]
                                    set_active: model.form.ssh_enabled,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_active_notify[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshEnabledChanged(row.is_active()));
                                    },
                                },

                                adw::EntryRow {
                                    set_title: &gettext("SSH Host"),
                                    set_text: &model.form.ssh_host,
                                    #[watch]
                                    set_visible: model.form.ssh_enabled,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_changed[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshHostChanged(row.text().to_string()));
                                    },
                                },

                                adw::EntryRow {
                                    set_title: &gettext("SSH Port"),
                                    set_text: &model.form.ssh_port,
                                    set_input_purpose: gtk::InputPurpose::Digits,
                                    #[watch]
                                    set_visible: model.form.ssh_enabled,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_changed[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshPortChanged(row.text().to_string()));
                                    },
                                },

                                adw::EntryRow {
                                    set_title: &gettext("SSH Username"),
                                    set_text: &model.form.ssh_username,
                                    #[watch]
                                    set_visible: model.form.ssh_enabled,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_changed[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshUsernameChanged(row.text().to_string()));
                                    },
                                },

                                adw::ComboRow {
                                    set_title: &gettext("Authentication"),
                                    set_model: Some(&model.ssh_auth_method_model),
                                    #[watch]
                                    set_selected: model.ssh_auth_method_index(),
                                    #[watch]
                                    set_visible: model.form.ssh_enabled,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_selected_notify[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshAuthMethodSelected(row.selected()));
                                    },
                                },

                                adw::PasswordEntryRow {
                                    set_title: &gettext("SSH Password"),
                                    set_text: &model.form.ssh_password,
                                    #[watch]
                                    set_visible: model.form.ssh_enabled && model.form.ssh_auth_method == SshAuthMethod::Password,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_changed[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshPasswordChanged(row.text().to_string()));
                                    },
                                },

                                adw::ActionRow {
                                    set_title: &gettext("Private Key File"),
                                    #[watch]
                                    set_subtitle: &model.private_key_path_subtitle(),
                                    #[watch]
                                    set_visible: model.form.ssh_enabled && model.form.ssh_auth_method == SshAuthMethod::PrivateKey,
                                    #[watch]
                                    set_sensitive: !model.is_busy,

                                    add_suffix = &gtk::Button {
                                        set_label: &gettext("Choose..."),
                                        set_valign: gtk::Align::Center,
                                        #[watch]
                                        set_sensitive: !model.is_busy,
                                        connect_clicked => ConnectionDialogMsg::SelectSshPrivateKeyFile,
                                    },
                                },

                                adw::PasswordEntryRow {
                                    set_title: &gettext("Key Passphrase"),
                                    set_text: &model.form.ssh_key_passphrase,
                                    #[watch]
                                    set_visible: model.form.ssh_enabled && model.form.ssh_auth_method == SshAuthMethod::PrivateKey,
                                    #[watch]
                                    set_sensitive: !model.is_busy,
                                    connect_changed[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshKeyPassphraseChanged(row.text().to_string()));
                                    },
                                },

                                adw::SwitchRow {
                                    set_title: &gettext("Save SSH Secret"),
                                    #[watch]
                                    set_subtitle: &model.save_ssh_secret_subtitle(),
                                    #[watch]
                                    set_visible: model.form.ssh_enabled && model.form.ssh_auth_method != SshAuthMethod::Agent,
                                    #[watch]
                                    set_active: model.form.ssh_save_secret,
                                    #[watch]
                                    set_sensitive: !model.is_busy && model.can_save_ssh_secret(),
                                    connect_active_notify[sender] => move |row| {
                                        sender.input(ConnectionDialogMsg::SshSaveSecretChanged(row.is_active()));
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
                        }
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
            persist_trusted_host_key_after_test: init.connection.is_some(),
            form: init
                .connection
                .as_ref()
                .map(ConnectionForm::from_saved)
                .unwrap_or_default(),
            ssh_auth_method_model: ssh_auth_method_model(),
            is_busy: false,
            saved_password_state: SavedPasswordState::Unknown,
            credential_state: CredentialState::Checking,
            pending_trusted_host_key_save: false,
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
        root: &Self::Root,
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
            ConnectionDialogMsg::SshEnabledChanged(value) => self.form.ssh_enabled = value,
            ConnectionDialogMsg::SshHostChanged(value) => {
                if self.form.ssh_host != value {
                    self.form.ssh_host_key_fingerprint = None;
                    self.pending_trusted_host_key_save = false;
                }
                self.form.ssh_host = value;
            }
            ConnectionDialogMsg::SshPortChanged(value) => {
                if self.form.ssh_port != value {
                    self.form.ssh_host_key_fingerprint = None;
                    self.pending_trusted_host_key_save = false;
                }
                self.form.ssh_port = value;
            }
            ConnectionDialogMsg::SshUsernameChanged(value) => self.form.ssh_username = value,
            ConnectionDialogMsg::SshAuthMethodSelected(index) => {
                self.form.ssh_auth_method = ssh_auth_method_from_index(index);
            }
            ConnectionDialogMsg::SshPasswordChanged(value) => self.form.ssh_password = value,
            ConnectionDialogMsg::SelectSshPrivateKeyFile => {
                show_private_key_file_dialog(root, &sender, self.form.ssh_private_key_path.clone());
            }
            ConnectionDialogMsg::SshPrivateKeyFileSelected(value) => {
                self.form.ssh_private_key_path = value;
            }
            ConnectionDialogMsg::SshKeyPassphraseChanged(value) => {
                self.form.ssh_key_passphrase = value;
            }
            ConnectionDialogMsg::SshSaveSecretChanged(value) => {
                self.form.ssh_save_secret = value && self.can_save_ssh_secret();
            }
            ConnectionDialogMsg::TrustSshHostKeyAndConnect(fingerprint) => {
                self.form.ssh_host_key_fingerprint = Some(fingerprint);
                sender.input(ConnectionDialogMsg::Connect);
            }
            ConnectionDialogMsg::TrustSshHostKeyAndTest(fingerprint) => {
                self.form.ssh_host_key_fingerprint = Some(fingerprint);
                self.pending_trusted_host_key_save = true;
                sender.input(ConnectionDialogMsg::TestConnection);
            }

            ConnectionDialogMsg::TestConnection => {
                let Some(details) = self.validated_details(widgets) else {
                    return;
                };

                self.ensure_form_id(&details);
                self.is_busy = true;
                sender.oneshot_command(async move {
                    ConnectionDialogCommandOutput::TestFinished(test_connection(details).await)
                });
            }

            ConnectionDialogMsg::Connect => {
                let Some(details) = self.validated_details(widgets) else {
                    return;
                };

                self.ensure_form_id(&details);
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
                self.form.ssh_save_secret = false;
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
                self.save_trusted_host_key_after_test(root);
                if let Some(toast_overlay) = root.toast_overlay() {
                    toast_overlay
                        .add_toast(adw::Toast::new(&gettext("Connection test succeeded.")));
                }
            }

            ConnectionDialogCommandOutput::TestFinished(Err(error)) => match error {
                ConnectionDialogError::UntrustedSshHostKey(fingerprint) => {
                    show_trust_ssh_host_key_dialog(
                        root,
                        &sender,
                        fingerprint,
                        SshHostKeyAction::Test,
                    );
                }
                ConnectionDialogError::Message(error) => {
                    show_error_dialog(root, &gettext("Connection test failed"), &error);
                }
            },

            ConnectionDialogCommandOutput::ConnectFinished(Ok(connected)) => {
                let _ = sender.output(ConnectionDialogOutput::Connected(connected));
                root.close();
            }

            ConnectionDialogCommandOutput::ConnectFinished(Err(error)) => match error {
                ConnectionDialogError::UntrustedSshHostKey(fingerprint) => {
                    show_trust_ssh_host_key_dialog(
                        root,
                        &sender,
                        fingerprint,
                        SshHostKeyAction::Connect,
                    );
                }
                ConnectionDialogError::Message(error) => {
                    show_error_dialog(root, &gettext("Connection failed"), &error);
                }
            },
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
            && (!self.form.ssh_enabled
                || (!self.form.ssh_host.trim().is_empty()
                    && self.form.ssh_port.trim().parse::<u16>().is_ok()
                    && !self.form.ssh_username.trim().is_empty()
                    && match self.form.ssh_auth_method {
                        SshAuthMethod::Password | SshAuthMethod::Agent => true,
                        SshAuthMethod::PrivateKey => {
                            !self.form.ssh_private_key_path.trim().is_empty()
                        }
                    }))
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
            CredentialState::Available => gettext("Store this password in Keyring."),
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
                gettext("Saved in Keyring. Enter a new password to replace it.")
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

    fn can_save_ssh_secret(&self) -> bool {
        self.can_save_password() && self.form.ssh_auth_method != SshAuthMethod::Agent
    }

    fn ssh_auth_method_index(&self) -> u32 {
        match self.form.ssh_auth_method {
            SshAuthMethod::Password => 0,
            SshAuthMethod::PrivateKey => 1,
            SshAuthMethod::Agent => 2,
        }
    }

    fn ensure_form_id(&mut self, details: &ConnectionDetails) {
        if self.form.id.is_none() {
            self.form.id = Some(details.saved.id.clone());
        }
    }

    fn private_key_path_subtitle(&self) -> String {
        if self.form.ssh_private_key_path.is_empty() {
            return gettext("No private key selected.");
        }

        self.form.ssh_private_key_path.clone()
    }

    fn save_trusted_host_key_after_test(&mut self, root: &adw::Window) {
        if !self.pending_trusted_host_key_save {
            return;
        }

        self.pending_trusted_host_key_save = false;

        if !self.persist_trusted_host_key_after_test {
            return;
        }

        let Ok(details) = self.form.validate() else {
            return;
        };

        if let Err(error) = connection_store::save_connection(&details.saved) {
            show_error_dialog(
                root,
                &gettext("Saving the trusted SSH host key failed"),
                &error.to_string(),
            );
        }
    }

    fn save_ssh_secret_subtitle(&self) -> String {
        match &self.credential_state {
            CredentialState::Checking => gettext("Checking password storage availability."),
            CredentialState::Available => {
                gettext("Store the SSH password or key passphrase in Keyring.")
            }
            CredentialState::Unavailable(_) => gettext("Password storage is not available."),
        }
    }
}

fn ssh_auth_method_model() -> gtk::StringList {
    let password = SshAuthMethod::Password.label();
    let private_key = SshAuthMethod::PrivateKey.label();
    let agent = SshAuthMethod::Agent.label();

    gtk::StringList::new(&[password.as_str(), private_key.as_str(), agent.as_str()])
}

fn ssh_auth_method_from_index(index: u32) -> SshAuthMethod {
    match index {
        1 => SshAuthMethod::PrivateKey,
        2 => SshAuthMethod::Agent,
        _ => SshAuthMethod::Password,
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

async fn test_connection(details: ConnectionDetails) -> Result<(), ConnectionDialogError> {
    let details = details_with_saved_password(details).await?;
    db::postgres::test_connection(&details)
        .await
        .map_err(connection_error)
}

async fn connect(
    details: ConnectionDetails,
) -> Result<Box<ConnectionDialogConnected>, ConnectionDialogError> {
    let details = details_with_saved_password(details).await?;
    let connection = db::postgres::connect(&details)
        .await
        .map_err(connection_error)?;

    let connected = ConnectionDialogConnected {
        details,
        connection,
    };

    Ok(Box::new(connected))
}

async fn details_with_saved_password(
    mut details: ConnectionDetails,
) -> Result<ConnectionDetails, ConnectionDialogError> {
    if details.password.is_empty()
        && details.saved.save_password
        && let Some(password) = credential_store::load_password(&details.saved.id)
            .await
            .map_err(|error| {
                ConnectionDialogError::Message(format!(
                    "{}: {error}",
                    gettext("Reading the saved password failed")
                ))
            })?
    {
        details.password = password;
    }

    if let Some(config) = details
        .saved
        .ssh_tunnel
        .as_ref()
        .filter(|config| config.save_secret)
    {
        match config.auth_method {
            SshAuthMethod::Password if details.ssh_password.is_empty() => {
                if let Some(password) = credential_store::load_ssh_password(&details.saved.id)
                    .await
                    .map_err(|error| {
                        ConnectionDialogError::Message(format!(
                            "{}: {error}",
                            gettext("Reading the saved SSH secret failed")
                        ))
                    })?
                {
                    details.ssh_password = password;
                }
            }
            SshAuthMethod::PrivateKey if details.ssh_key_passphrase.is_empty() => {
                if let Some(passphrase) =
                    credential_store::load_ssh_key_passphrase(&details.saved.id)
                        .await
                        .map_err(|error| {
                            ConnectionDialogError::Message(format!(
                                "{}: {error}",
                                gettext("Reading the saved SSH secret failed")
                            ))
                        })?
                {
                    details.ssh_key_passphrase = passphrase;
                }
            }
            _ => {}
        }
    }

    Ok(details)
}

fn connection_error(error: db::postgres::PostgresError) -> ConnectionDialogError {
    match error {
        db::postgres::PostgresError::SshTunnel(
            db::ssh_tunnel::SshTunnelError::UntrustedHostKey(fingerprint),
        ) => ConnectionDialogError::UntrustedSshHostKey(fingerprint),
        other => ConnectionDialogError::Message(other.to_string()),
    }
}

#[derive(Debug, Clone, Copy)]
enum SshHostKeyAction {
    Connect,
    Test,
}

fn show_trust_ssh_host_key_dialog(
    parent: &adw::Window,
    sender: &ComponentSender<ConnectionDialog>,
    fingerprint: String,
    action: SshHostKeyAction,
) {
    let body = format!(
        "{}\n\n{}",
        gettext("This SSH host key has not been seen before. Trust it for this connection?"),
        fingerprint
    );
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Trust SSH Host Key?"))
        .body(body)
        .close_response("cancel")
        .build();

    dialog.add_response("cancel", &gettext("Cancel"));
    dialog.add_response("trust", &gettext("Trust"));
    dialog.set_response_appearance("trust", adw::ResponseAppearance::Suggested);

    let input_sender = sender.input_sender().clone();
    dialog.connect_response(None, move |_dialog, response| {
        if response != "trust" {
            return;
        }

        let msg = match action {
            SshHostKeyAction::Connect => {
                ConnectionDialogMsg::TrustSshHostKeyAndConnect(fingerprint.clone())
            }
            SshHostKeyAction::Test => {
                ConnectionDialogMsg::TrustSshHostKeyAndTest(fingerprint.clone())
            }
        };

        let _ = input_sender.send(msg);
    });

    dialog.present(Some(parent));
}

fn show_private_key_file_dialog(
    parent: &adw::Window,
    sender: &ComponentSender<ConnectionDialog>,
    current_path: String,
) {
    let dialog = gtk::FileChooserNative::new(
        Some(&gettext("Select Private Key")),
        Some(parent),
        gtk::FileChooserAction::Open,
        Some(&gettext("Select")),
        Some(&gettext("Cancel")),
    );

    dialog.set_modal(true);

    if !current_path.is_empty() {
        let file = gtk::gio::File::for_path(&current_path);
        let _ = dialog.set_file(&file);
    }

    let input_sender = sender.input_sender().clone();
    dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            return;
        }

        if let Some(path) = dialog.file().and_then(|file| file.path()) {
            let _ = input_sender.send(ConnectionDialogMsg::SshPrivateKeyFileSelected(
                path.to_string_lossy().into_owned(),
            ));
        }
    });

    dialog.show();
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
