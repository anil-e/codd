use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::settings;
use crate::window_content::{WindowContent, WindowContentMsg};

pub struct AppWindow {
    content: Controller<WindowContent>,
}

#[derive(Debug)]
pub enum AppWindowMsg {
    OpenConnectionDialog,
    RunQuery,
    FocusEditor,
    Quit,
}

#[relm4::component(pub)]
impl Component for AppWindow {
    type Init = ();
    type Input = AppWindowMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        adw::ApplicationWindow {
            set_title: Some("Codd"),
            set_default_size: (1180, 760),
            set_width_request: 360,
            set_height_request: 320,

            #[wrap(Some)]
            set_content = model.content.widget(),
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        sourceview5::init();

        let app_settings = settings::app_settings();
        let content = WindowContent::builder().launch(()).detach();
        let model = AppWindow { content };
        let widgets = view_output!();

        let width = app_settings.int("window-width");
        let height = app_settings.int("window-height");
        root.set_default_width(width);
        root.set_default_height(height);
        root.connect_close_request(move |window| {
            let settings = settings::app_settings();
            let _ = settings.set_int("window-width", window.width());
            let _ = settings.set_int("window-height", window.height());
            glib::Propagation::Proceed
        });

        setup_window_actions(&root, &sender);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            AppWindowMsg::OpenConnectionDialog => {
                self.content.emit(WindowContentMsg::OpenConnectionDialog);
            }
            AppWindowMsg::RunQuery => {
                self.content.emit(WindowContentMsg::RunQuery);
            }
            AppWindowMsg::FocusEditor => {
                self.content.emit(WindowContentMsg::FocusEditor);
            }
            AppWindowMsg::Quit => {
                relm4::main_adw_application().quit();
            }
        }
    }
}

fn setup_window_actions(root: &adw::ApplicationWindow, sender: &ComponentSender<AppWindow>) {
    let app = relm4::main_adw_application();

    let action = gtk::gio::SimpleAction::new("new-connection", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::OpenConnectionDialog);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("run-query", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::RunQuery);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("focus-editor", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::FocusEditor);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("quit", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::Quit);
    });
    app.add_action(&action);

    app.set_accels_for_action("win.new-connection", &["<Control>n"]);
    app.set_accels_for_action("win.run-query", &["<Control>Return"]);
    app.set_accels_for_action("win.focus-editor", &["<Control>e"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}
