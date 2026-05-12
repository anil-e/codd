use crate::db;
use crate::models::connection::{ConnectionDetails, ConnectionForm, SavedConnection};
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
}

#[derive(Debug)]
pub enum ConnectionDialogMsg {
    NameChanged(String),
    HostChanged(String),
    PortChanged(String),
    DatabaseChanged(String),
    UsernameChanged(String),
    PasswordChanged(String),
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
    TestFinished(Result<(), String>),
    ConnectFinished {
        details: ConnectionDetails,
        result: Result<PgPool, String>,
    },
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
        };
        let widgets = view_output!();

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

            ConnectionDialogMsg::TestConnection => {
                let Some(details) = self.validated_details(widgets) else {
                    return;
                };

                self.is_busy = true;
                sender.oneshot_command(async move {
                    ConnectionDialogCommandOutput::TestFinished(
                        test_connection(details)
                            .await
                            .map_err(|error| error.to_string()),
                    )
                });
            }

            ConnectionDialogMsg::Connect => {
                let Some(details) = self.validated_details(widgets) else {
                    return;
                };

                self.is_busy = true;
                sender.oneshot_command(async move {
                    ConnectionDialogCommandOutput::ConnectFinished {
                        details: details.clone(),
                        result: connect(details).await.map_err(|error| error.to_string()),
                    }
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
            ConnectionDialogCommandOutput::TestFinished(Ok(())) => {
                if let Some(toast_overlay) = root.toast_overlay() {
                    toast_overlay
                        .add_toast(adw::Toast::new(&gettext("Connection test succeeded.")));
                }
            }

            ConnectionDialogCommandOutput::TestFinished(Err(error)) => {
                show_error_dialog(root, &gettext("Connection test failed"), &error);
            }

            ConnectionDialogCommandOutput::ConnectFinished {
                details,
                result: Ok(pool),
            } => {
                let _ = sender.output(ConnectionDialogOutput::Connected { details, pool });
                root.close();
            }

            ConnectionDialogCommandOutput::ConnectFinished {
                result: Err(error), ..
            } => {
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
}

trait WindowToastOverlay {
    fn toast_overlay(&self) -> Option<adw::ToastOverlay>;
}

impl WindowToastOverlay for adw::Window {
    fn toast_overlay(&self) -> Option<adw::ToastOverlay> {
        self.content().and_downcast::<adw::ToastOverlay>()
    }
}

async fn test_connection(details: ConnectionDetails) -> Result<(), db::postgres::PostgresError> {
    db::postgres::test_connection(&details).await
}

async fn connect(details: ConnectionDetails) -> Result<PgPool, db::postgres::PostgresError> {
    db::postgres::connect(&details).await
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
