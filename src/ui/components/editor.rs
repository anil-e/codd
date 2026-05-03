use gettextrs::gettext;
use libadwaita as adw;
use relm4::gtk;
use relm4::gtk::glib;
use relm4::gtk::prelude::*;
use relm4::prelude::*;
use sourceview5::prelude::*;

pub struct SqlEditor {
    buffer: sourceview5::Buffer,
    is_running: bool,
    style_manager: adw::StyleManager,
    dark_notify_handler: Option<glib::SignalHandlerId>,
}

#[derive(Debug)]
pub enum SqlEditorMsg {
    SetRunning(bool),
    Focus,
    RunRequested,
    CancelRequested,
}

#[derive(Debug)]
pub enum SqlEditorOutput {
    RunRequested,
    CancelRequested,
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

                gtk::Box {
                    set_hexpand: true,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Execute Statement")),
                    add_css_class: "suggested-action",
                    set_child: Some(&adw::ButtonContent::builder()
                        .icon_name("media-playback-start-symbolic")
                        .label(gettext("Execute Statement"))
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
        let style_manager = adw::StyleManager::default();
        apply_style_scheme(&buffer, style_manager.is_dark());
        let dark_notify_handler = {
            let buffer = buffer.clone();
            style_manager.connect_dark_notify(move |style_manager| {
                apply_style_scheme(&buffer, style_manager.is_dark());
            })
        };

        let model = SqlEditor {
            buffer,
            is_running: false,
            style_manager,
            dark_notify_handler: Some(dark_notify_handler),
        };
        let widgets = view_output!();

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
        }

        self.update_view(widgets, sender);
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        if let Some(handler) = self.dark_notify_handler.take() {
            self.style_manager.disconnect(handler);
        }
    }
}

fn configure_sql_buffer(buffer: &sourceview5::Buffer) {
    buffer.set_highlight_syntax(true);
    buffer.set_highlight_matching_brackets(true);

    if let Some(language) = sourceview5::LanguageManager::default().language("sql") {
        buffer.set_language(Some(&language));
    }
}

fn apply_style_scheme(buffer: &sourceview5::Buffer, is_dark: bool) {
    let scheme_id = if is_dark { "Adwaita-dark" } else { "Adwaita" };

    if let Some(scheme) = sourceview5::StyleSchemeManager::default().scheme(scheme_id) {
        buffer.set_style_scheme(Some(&scheme));
    }
}
