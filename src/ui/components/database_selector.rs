use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

pub struct DatabaseSelector {
    connection_title: String,
    active_database: String,
    databases: Vec<String>,
    search_text: String,
    is_loading: bool,
}

#[derive(Debug)]
pub enum DatabaseSelectorMsg {
    SetContext {
        connection_title: String,
        active_database: String,
        databases: Vec<String>,
    },
    SetDatabases(Vec<String>),
    SetLoading(bool),
    SearchChanged(String),
    DatabaseRowActivated(usize),
}

#[derive(Debug)]
pub enum DatabaseSelectorOutput {
    DatabaseSelected(String),
}

#[relm4::component(pub)]
impl Component for DatabaseSelector {
    type Init = ();
    type Input = DatabaseSelectorMsg;
    type Output = DatabaseSelectorOutput;
    type CommandOutput = ();

    view! {
        gtk::Stack {
            add_named[Some("title")] = &adw::WindowTitle {
                set_title: &gettext("Codd"),
            },

            add_named[Some("selector")] = &gtk::MenuButton {
                add_css_class: "flat",
                set_tooltip_text: Some(&gettext("Switch database")),
                #[watch]
                set_visible: model.has_context(),
                #[watch]
                set_sensitive: !model.is_loading && !model.databases.is_empty(),
                #[wrap(Some)]
                set_child = &gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 8,

                    gtk::Image {
                        set_icon_name: Some("network-server-symbolic"),
                    },

                    gtk::Label {
                        #[watch]
                        set_label: &model.active_database_label(),
                    },

                    gtk::Image {
                        set_icon_name: Some("pan-down-symbolic"),
                        add_css_class: "dim-label",
                    },
                },

                #[wrap(Some)]
                #[name = "database_popover"]
                set_popover = &gtk::Popover {
                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 8,
                        set_margin_top: 10,
                        set_margin_bottom: 10,
                        set_margin_start: 10,
                        set_margin_end: 10,
                        set_width_request: 320,

                        gtk::Label {
                            #[watch]
                            set_label: &model.connection_title,
                            add_css_class: "heading",
                            set_halign: gtk::Align::Start,
                            #[watch]
                            set_visible: !model.connection_title.is_empty(),
                        },

                        #[name = "search_entry"]
                        gtk::SearchEntry {
                            set_placeholder_text: Some(&gettext("Search databases")),
                            #[watch]
                            set_sensitive: !model.is_loading,
                            connect_search_changed[sender] => move |entry| {
                                sender.input(DatabaseSelectorMsg::SearchChanged(entry.text().to_string()));
                            },
                        },

                        gtk::ScrolledWindow {
                            set_min_content_height: 180,
                            set_max_content_height: 320,
                            set_policy: (gtk::PolicyType::Never, gtk::PolicyType::Automatic),

                            #[name = "database_list"]
                            gtk::ListBox {
                                set_selection_mode: gtk::SelectionMode::None,
                                connect_row_activated[sender] => move |_, row| {
                                    sender.input(DatabaseSelectorMsg::DatabaseRowActivated(row.index() as usize));
                                },
                            },
                        },

                        gtk::Label {
                            set_label: &gettext("No databases match your search."),
                            add_css_class: "dim-label",
                            set_halign: gtk::Align::Center,
                            #[watch]
                            set_visible: model.has_active_search() && !model.has_search_results(),
                        },
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = DatabaseSelector {
            connection_title: String::new(),
            active_database: String::new(),
            databases: Vec::new(),
            search_text: String::new(),
            is_loading: false,
        };
        let widgets = view_output!();

        root.set_visible_child_name("title");
        model.render_databases(&widgets);

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
            DatabaseSelectorMsg::SetContext {
                connection_title,
                active_database,
                databases,
            } => {
                self.connection_title = connection_title;
                self.active_database = active_database;
                self.databases = databases;
                self.search_text.clear();
                widgets.search_entry.set_text("");
                self.render_databases(widgets);
            }

            DatabaseSelectorMsg::SetDatabases(databases) => {
                self.databases = databases;
                self.render_databases(widgets);
            }

            DatabaseSelectorMsg::SetLoading(is_loading) => {
                self.is_loading = is_loading;
            }

            DatabaseSelectorMsg::SearchChanged(text) => {
                self.search_text = text;
                self.render_databases(widgets);
            }

            DatabaseSelectorMsg::DatabaseRowActivated(index) => {
                if let Some(database) = self.filtered_databases().get(index)
                    && *database != self.active_database
                {
                    widgets.database_popover.popdown();
                    let _ = sender.output(DatabaseSelectorOutput::DatabaseSelected(
                        (*database).to_string(),
                    ));
                }
            }
        }

        root.set_visible_child_name(if self.has_context() {
            "selector"
        } else {
            "title"
        });
        self.update_view(widgets, sender);
    }
}

impl DatabaseSelector {
    fn has_context(&self) -> bool {
        !self.active_database.is_empty()
    }

    fn active_database_label(&self) -> String {
        if self.is_loading {
            gettext("Loading...")
        } else {
            self.active_database.clone()
        }
    }

    fn has_active_search(&self) -> bool {
        !self.search_text.trim().is_empty()
    }

    fn has_search_results(&self) -> bool {
        !self.filtered_databases().is_empty()
    }

    fn filtered_databases(&self) -> Vec<&str> {
        let search = self.search_text.trim().to_lowercase();

        self.databases
            .iter()
            .filter(|database| search.is_empty() || database.to_lowercase().contains(&search))
            .map(String::as_str)
            .collect()
    }

    fn render_databases(&self, widgets: &DatabaseSelectorWidgets) {
        clear_list(&widgets.database_list);

        for database in self.filtered_databases() {
            let row = gtk::ListBoxRow::builder()
                .activatable(database != self.active_database)
                .build();
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            content.set_margin_top(4);
            content.set_margin_bottom(4);
            content.set_margin_start(8);
            content.set_margin_end(8);

            let label = gtk::Label::new(Some(database));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            content.append(&label);

            if database == self.active_database {
                row.add_css_class("accent");
                content.append(
                    &gtk::Image::builder()
                        .icon_name("object-select-symbolic")
                        .css_classes(["dim-label"])
                        .build(),
                );
            }

            row.set_child(Some(&content));

            widgets.database_list.append(&row);
        }
    }
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
}
