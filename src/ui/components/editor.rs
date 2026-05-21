use crate::models::query_history::QueryHistoryEntry;
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::prelude::*;
use sourceview5::prelude::*;

pub struct SqlEditor {
    buffer: sourceview5::Buffer,
    history_preview_buffer: sourceview5::Buffer,
    is_running: bool,
    has_selection: bool,
    history: Vec<QueryHistoryEntry>,
    selected_history_index: Option<usize>,
    style_manager: adw::StyleManager,
    dark_notify_handler: Option<glib::SignalHandlerId>,
    selection_notify_handler: Option<glib::SignalHandlerId>,
}

#[derive(Debug)]
pub enum SqlEditorMsg {
    SetRunning(bool),
    SelectionChanged(bool),
    SetHistory(Vec<QueryHistoryEntry>),
    Focus,
    RunRequested,
    CancelRequested,
    HistoryRowSelected(usize),
    InsertSelectedHistoryRequested,
    ClearHistoryRequested,
}

#[derive(Debug)]
pub enum SqlEditorOutput {
    RunRequested,
    CancelRequested,
    HistorySelected(String),
    ClearHistoryRequested,
}

#[relm4::component(pub)]
impl Component for SqlEditor {
    type Init = sourceview5::Buffer;
    type Input = SqlEditorMsg;
    type Output = SqlEditorOutput;
    type CommandOutput = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 0,
            add_css_class: "editor-pane",

            gtk::ScrolledWindow {
                set_hexpand: true,
                set_vexpand: true,
                set_margin_top: 12,
                set_margin_bottom: 0,
                set_margin_start: 12,
                set_margin_end: 12,
                add_css_class: "editor-scroller",

                #[name = "source_view"]
                sourceview5::View {
                    set_buffer: Some(&model.buffer),
                    set_monospace: true,
                    set_show_line_numbers: true,
                    set_highlight_current_line: true,
                    set_insert_spaces_instead_of_tabs: true,
                    set_tab_width: 4,
                    set_top_margin: 18,
                    set_bottom_margin: 18,
                    set_left_margin: 18,
                    set_right_margin: 18,

                    add_controller = gtk::EventControllerKey {
                        connect_key_pressed[sender] => move |_, key, _, modifiers| {
                            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                                && (key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter)
                            {
                                sender.input(SqlEditorMsg::RunRequested);
                                glib::Propagation::Stop
                            } else {
                                glib::Propagation::Proceed
                            }
                        },
                    },
                },
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 12,
                set_margin_top: 8,
                set_margin_bottom: 12,
                set_margin_start: 12,
                set_margin_end: 12,
                add_css_class: "editor-action-bar",

                #[name = "history_button"]
                gtk::MenuButton {
                    set_tooltip_text: Some(&gettext("Query History")),
                    add_css_class: "flat",
                    set_child: Some(&adw::ButtonContent::builder()
                        .icon_name("document-open-symbolic")
                        .label(gettext("History"))
                        .build()
                    ),

                    #[wrap(Some)]
                    set_popover = &gtk::Popover {
                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_size_request: (720, 460),

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 0,
                                set_margin_top: 10,
                                set_margin_bottom: 0,
                                set_margin_start: 10,
                                set_margin_end: 10,
                                set_vexpand: true,

                                gtk::ScrolledWindow {
                                    set_width_request: 240,
                                    set_vexpand: true,
                                    set_min_content_height: 340,
                                    set_max_content_height: 340,
                                    set_policy: (gtk::PolicyType::Never, gtk::PolicyType::Automatic),

                                    #[name = "history_list"]
                                    gtk::ListBox {
                                        set_selection_mode: gtk::SelectionMode::Single,
                                        add_css_class: "boxed-list",
                                        connect_row_selected[sender] => move |_, row| {
                                            if let Some(row) = row {
                                                sender.input(SqlEditorMsg::HistoryRowSelected(row.index() as usize));
                                            }
                                        },
                                    },
                                },

                                gtk::Separator {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_margin_start: 10,
                                    set_margin_end: 10,
                                },

                                gtk::ScrolledWindow {
                                    set_hexpand: true,
                                    set_vexpand: true,
                                    set_min_content_width: 360,
                                    set_min_content_height: 340,
                                    set_max_content_height: 340,
                                    set_policy: (gtk::PolicyType::Never, gtk::PolicyType::Automatic),

                                    #[name = "history_preview"]
                                    sourceview5::View {
                                        set_buffer: Some(&model.history_preview_buffer),
                                        set_editable: false,
                                        set_cursor_visible: false,
                                        set_monospace: true,
                                        set_wrap_mode: gtk::WrapMode::WordChar,
                                        set_show_line_numbers: false,
                                        set_highlight_current_line: false,
                                        set_top_margin: 0,
                                        set_bottom_margin: 8,
                                        set_left_margin: 12,
                                        set_right_margin: 12,
                                        add_css_class: "monospace",
                                        add_css_class: "history-preview",
                                    },
                                },
                            },

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 8,
                                set_margin_top: 8,
                                set_margin_bottom: 8,
                                set_margin_start: 10,
                                set_margin_end: 10,

                                gtk::Button {
                                    set_label: &gettext("Clear"),
                                    add_css_class: "flat",
                                    add_css_class: "destructive-action",
                                    #[watch]
                                    set_sensitive: !model.history.is_empty(),
                                    connect_clicked => SqlEditorMsg::ClearHistoryRequested,
                                },

                                gtk::Box {
                                    set_hexpand: true,
                                },

                                gtk::Button {
                                    set_label: &gettext("Apply"),
                                    add_css_class: "suggested-action",
                                    #[watch]
                                    set_sensitive: model.selected_history_index.is_some(),
                                    connect_clicked => SqlEditorMsg::InsertSelectedHistoryRequested,
                                },
                            },
                        },
                    },
                },

                gtk::Box {
                    set_hexpand: true,
                },

                gtk::Label {
                    set_label: &gettext("Selection will be executed"),
                    add_css_class: "caption",
                    add_css_class: "dim-label",
                    set_width_chars: 28,
                    set_xalign: 1.0,
                    #[watch]
                    set_visible: model.has_selection && !model.is_running,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Execute selection or statement")),
                    add_css_class: "suggested-action",
                    set_child: Some(&adw::ButtonContent::builder()
                        .icon_name("media-playback-start-symbolic")
                        .label(gettext("Execute"))
                        .build()
                    ),
                    #[watch]
                    set_visible: !model.is_running,
                    #[watch]
                    set_sensitive: !model.is_running,
                    connect_clicked => SqlEditorMsg::RunRequested,
                },

                gtk::Button {
                    set_label: &gettext("Cancel"),
                    add_css_class: "destructive-action",
                    #[watch]
                    set_visible: model.is_running,
                    connect_clicked => SqlEditorMsg::CancelRequested,
                },
            },
        }
    }

    fn init(
        buffer: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        configure_sql_buffer(&buffer);
        let history_preview_buffer = sourceview5::Buffer::new(None);
        configure_sql_buffer(&history_preview_buffer);
        let style_manager = adw::StyleManager::default();
        apply_style_scheme(&buffer, &history_preview_buffer, style_manager.is_dark());
        let dark_notify_handler = {
            let buffer = buffer.clone();
            let history_preview_buffer = history_preview_buffer.clone();
            style_manager.connect_dark_notify(move |style_manager| {
                apply_style_scheme(&buffer, &history_preview_buffer, style_manager.is_dark());
            })
        };
        let selection_notify_handler = {
            let sender = sender.clone();
            buffer.connect_has_selection_notify(move |buffer| {
                sender.input(SqlEditorMsg::SelectionChanged(buffer.has_selection()));
            })
        };

        let mut model = SqlEditor {
            buffer,
            history_preview_buffer,
            is_running: false,
            has_selection: false,
            history: Vec::new(),
            selected_history_index: None,
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
            selection_notify_handler: Some(selection_notify_handler),
        };
        let widgets = view_output!();
        model.render_history(&widgets);

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
            SqlEditorMsg::SetRunning(is_running) => {
                self.is_running = is_running;
            }

            SqlEditorMsg::SelectionChanged(has_selection) => {
                self.has_selection = has_selection;
            }

            SqlEditorMsg::SetHistory(history) => {
                self.history = history;
                self.selected_history_index = self.default_history_selection();
                self.sync_history_preview();
                self.render_history(widgets);
            }

            SqlEditorMsg::Focus => {
                widgets.source_view.grab_focus();
            }

            SqlEditorMsg::RunRequested => {
                if !self.is_running {
                    let _ = sender.output(SqlEditorOutput::RunRequested);
                }
            }

            SqlEditorMsg::CancelRequested => {
                let _ = sender.output(SqlEditorOutput::CancelRequested);
            }

            SqlEditorMsg::HistoryRowSelected(index) => {
                self.selected_history_index = self.history.get(index).map(|_| index);
                self.sync_history_preview();
            }

            SqlEditorMsg::InsertSelectedHistoryRequested => {
                if let Some(entry) = self.selected_history_entry() {
                    let _ = sender.output(SqlEditorOutput::HistorySelected(entry.sql.clone()));
                    widgets.history_button.popdown();
                }
            }

            SqlEditorMsg::ClearHistoryRequested => {
                let _ = sender.output(SqlEditorOutput::ClearHistoryRequested);
            }
        }

        self.update_view(widgets, sender);
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        if let Some(handler) = self.dark_notify_handler.take() {
            self.style_manager.disconnect(handler);
        }

        if let Some(handler) = self.selection_notify_handler.take() {
            self.buffer.disconnect(handler);
        }
    }
}

impl SqlEditor {
    fn render_history(&mut self, widgets: &SqlEditorWidgets) {
        clear_history_list(&widgets.history_list);

        if self.history.is_empty() {
            self.sync_history_preview();
            let empty_row = gtk::ListBoxRow::builder()
                .activatable(false)
                .selectable(false)
                .child(
                    &gtk::Label::builder()
                        .label(gettext("No query history yet."))
                        .css_classes(["dim-label"])
                        .margin_top(18)
                        .margin_bottom(18)
                        .margin_start(18)
                        .margin_end(18)
                        .build(),
                )
                .build();
            widgets.history_list.append(&empty_row);
            return;
        }

        for entry in &self.history {
            let row = gtk::ListBoxRow::builder().activatable(true).build();
            row.set_child(Some(&history_list_row_content(entry)));

            widgets.history_list.append(&row);
        }

        if let Some(index) = self.selected_history_index
            && let Some(row) = widgets.history_list.row_at_index(index as i32)
        {
            widgets.history_list.select_row(Some(&row));
        }
    }

    fn default_history_selection(&self) -> Option<usize> {
        (!self.history.is_empty()).then_some(0)
    }

    fn selected_history_entry(&self) -> Option<&QueryHistoryEntry> {
        self.selected_history_index
            .and_then(|index| self.history.get(index))
    }

    fn sync_history_preview(&self) {
        let preview = self
            .selected_history_entry()
            .map(|entry| entry.sql.as_str())
            .unwrap_or_default();
        self.history_preview_buffer.set_text(preview);
    }
}

fn history_list_row_content(entry: &QueryHistoryEntry) -> gtk::Box {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(1)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(10)
        .margin_end(10)
        .build();

    content.append(
        &gtk::Label::builder()
            .label(history_subtitle(entry.executed_at))
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build(),
    );

    content.append(
        &gtk::Label::builder()
            .label(history_list_preview(&entry.sql))
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .single_line_mode(true)
            .max_width_chars(24)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["caption"])
            .build(),
    );

    content
}

fn clear_history_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn history_list_preview(sql: &str) -> String {
    let preview = sql
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    truncate_for_display(&preview, 80)
}

fn history_subtitle(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|date_time| {
            date_time
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| gettext("Unknown time"))
}

fn truncate_for_display(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();

    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn configure_sql_buffer(buffer: &sourceview5::Buffer) {
    buffer.set_highlight_syntax(true);
    buffer.set_highlight_matching_brackets(true);

    if let Some(language) = sourceview5::LanguageManager::default().language("sql") {
        buffer.set_language(Some(&language));
    }
}

fn apply_style_scheme(
    editor_buffer: &sourceview5::Buffer,
    history_preview_buffer: &sourceview5::Buffer,
    is_dark: bool,
) {
    let scheme_id = if is_dark { "Adwaita-dark" } else { "Adwaita" };

    if let Some(scheme) = sourceview5::StyleSchemeManager::default().scheme(scheme_id) {
        editor_buffer.set_style_scheme(Some(&scheme));
        history_preview_buffer.set_style_scheme(Some(&scheme));
    }
}
