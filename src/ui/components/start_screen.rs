use crate::models::connection::SavedConnection;
use crate::state::connection_store;
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

pub struct StartScreen {
    connections: Vec<SavedConnection>,
}

#[derive(Debug)]
pub enum StartScreenMsg {
    SetConnections(Vec<SavedConnection>),
    NewConnection,
    OpenConnection(SavedConnection),
    RequestRename(SavedConnection),
    RequestRemove(SavedConnection),
    RenameConfirmed { id: String, name: String },
    RemoveConfirmed(String),
}

#[derive(Debug)]
pub enum StartScreenOutput {
    NewConnection,
    OpenConnection(SavedConnection),
    ConnectionsChanged(Vec<SavedConnection>),
}

#[relm4::component(pub)]
impl Component for StartScreen {
    type Init = Vec<SavedConnection>;
    type Input = StartScreenMsg;
    type Output = StartScreenOutput;
    type CommandOutput = ();

    view! {
        adw::ToastOverlay {
            #[wrap(Some)]
            #[name = "stack"]
            set_child = &gtk::Stack {
                add_named[Some("empty")] = &adw::StatusPage {
                    set_icon_name: Some("database-regular"),
                    set_title: &gettext("Welcome to Codd"),
                    set_description: Some(&gettext("Add a PostgreSQL connection to get started")),

                    #[wrap(Some)]
                    set_child = &gtk::Button {
                        set_label: &gettext("Add Connection..."),
                        set_halign: gtk::Align::Center,
                        set_hexpand: false,
                        add_css_class: "pill",
                        add_css_class: "suggested-action",
                        connect_clicked => StartScreenMsg::NewConnection,
                    },
                },

                add_named[Some("list")] = &gtk::ScrolledWindow {
                    set_vexpand: true,

                    adw::Clamp {
                        set_maximum_size: 720,
                        set_margin_top: 24,
                        set_margin_bottom: 24,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 10,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 6,

                                gtk::Label {
                                    set_label: &gettext("Connections"),
                                    add_css_class: "title-4",
                                    set_halign: gtk::Align::Start,
                                    set_hexpand: true,
                                },

                                gtk::Button {
                                    set_icon_name: "add",
                                    set_tooltip_text: Some(&gettext("Add Connection...")),
                                    add_css_class: "flat",
                                    connect_clicked => StartScreenMsg::NewConnection,
                                },
                            },

                            #[name = "connection_list"]
                            gtk::ListBox {
                                set_selection_mode: gtk::SelectionMode::None,
                                add_css_class: "boxed-list",
                            },

                        },
                    },
                },
            },
        }
    }

    fn init(
        connections: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = StartScreen { connections };
        let widgets = view_output!();

        model.render(&widgets, &sender);

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
            StartScreenMsg::SetConnections(connections) => {
                self.connections = connections;
                self.render(widgets, &sender);
            }

            StartScreenMsg::NewConnection => {
                let _ = sender.output(StartScreenOutput::NewConnection);
            }

            StartScreenMsg::OpenConnection(connection) => {
                let _ = sender.output(StartScreenOutput::OpenConnection(connection));
            }

            StartScreenMsg::RequestRename(connection) => {
                show_rename_dialog(root, &sender, &connection);
            }

            StartScreenMsg::RequestRemove(connection) => {
                show_remove_dialog(root, &sender, &connection);
            }

            StartScreenMsg::RenameConfirmed { id, name } => {
                let name = name.trim();
                if name.is_empty() {
                    return;
                }

                match connection_store::rename_connection(&id, name) {
                    Ok(connections) => {
                        self.connections.clone_from(&connections);
                        self.render(widgets, &sender);
                        let _ = sender.output(StartScreenOutput::ConnectionsChanged(connections));
                    }
                    Err(error) => {
                        show_error_toast(
                            root,
                            format!("{}: {error}", gettext("Renaming the connection failed")),
                        );
                    }
                }
            }

            StartScreenMsg::RemoveConfirmed(id) => match connection_store::remove_connection(&id) {
                Ok(connections) => {
                    self.connections.clone_from(&connections);
                    self.render(widgets, &sender);
                    let _ = sender.output(StartScreenOutput::ConnectionsChanged(connections));
                }
                Err(error) => {
                    show_error_toast(
                        root,
                        format!("{}: {error}", gettext("Removing the connection failed")),
                    );
                }
            },
        }

        self.update_view(widgets, sender);
    }
}

impl StartScreen {
    fn render(&self, widgets: &StartScreenWidgets, sender: &ComponentSender<Self>) {
        clear_list(&widgets.connection_list);

        widgets
            .stack
            .set_visible_child_name(if self.connections.is_empty() {
                "empty"
            } else {
                "list"
            });

        for connection in &self.connections {
            let row = adw::ActionRow::builder()
                .title(&connection.name)
                .subtitle(format!(
                    "{}@{}:{}/{}",
                    connection.username, connection.host, connection.port, connection.database
                ))
                .activatable(true)
                .build();

            row.add_prefix(
                &gtk::Image::builder()
                    .icon_name("database-regular")
                    .css_classes(["dim-label"])
                    .build(),
            );

            let rename_button = gtk::Button::builder()
                .icon_name("edit-regular")
                .tooltip_text(gettext("Rename connection"))
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .build();
            let delete_button = gtk::Button::builder()
                .icon_name("delete-regular")
                .tooltip_text(gettext("Remove connection"))
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .build();

            let rename_target = connection.clone();
            let rename_sender = sender.clone();
            rename_button.connect_clicked(move |_| {
                rename_sender.input(StartScreenMsg::RequestRename(rename_target.clone()));
            });

            let delete_target = connection.clone();
            let delete_sender = sender.clone();
            delete_button.connect_clicked(move |_| {
                delete_sender.input(StartScreenMsg::RequestRemove(delete_target.clone()));
            });

            row.add_suffix(&rename_button);
            row.add_suffix(&delete_button);
            row.add_suffix(
                &gtk::Image::builder()
                    .icon_name("go-next")
                    .css_classes(["dim-label"])
                    .build(),
            );

            let selected = connection.clone();
            let sender = sender.clone();
            row.connect_activated(move |_| {
                sender.input(StartScreenMsg::OpenConnection(selected.clone()));
            });

            widgets.connection_list.append(&row);
        }
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn show_rename_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<StartScreen>,
    connection: &SavedConnection,
) {
    let entry = gtk::Entry::builder()
        .text(&connection.name)
        .activates_default(true)
        .hexpand(true)
        .build();

    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Rename Connection"))
        .body(gettext("Choose a new name for this connection."))
        .extra_child(&entry)
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("rename", &gettext("Rename")),
    ]);
    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);

    let sender = sender.clone();
    let id = connection.id.clone();
    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "rename" {
                sender.input(StartScreenMsg::RenameConfirmed {
                    id,
                    name: entry.text().to_string(),
                });
            }
        },
    );
}

fn show_remove_dialog(
    root: &adw::ToastOverlay,
    sender: &ComponentSender<StartScreen>,
    connection: &SavedConnection,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Remove Connection"))
        .body(format!(
            "{} “{}”?",
            gettext("Remove the saved connection"),
            connection.name
        ))
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("remove", &gettext("Remove")),
    ]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);

    let sender = sender.clone();
    let id = connection.id.clone();
    dialog.choose(
        root.root().as_ref(),
        None::<&gtk::gio::Cancellable>,
        move |response| {
            if response == "remove" {
                sender.input(StartScreenMsg::RemoveConfirmed(id));
            }
        },
    );
}

fn show_error_toast(root: &adw::ToastOverlay, message: String) {
    root.add_toast(adw::Toast::new(&message));
}
