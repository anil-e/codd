use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::settings;
use crate::window_content::{WindowContent, WindowContentMsg};

pub struct AppWindowInit {
    pub id: u64,
}

pub struct AppWindow {
    id: u64,
    content: Controller<WindowContent>,
}

#[derive(Debug)]
pub enum AppWindowMsg {
    OpenConnectionDialog,
    NewQueryTab,
    RunQuery,
    RefreshTableBrowser,
    FocusEditor,
    FocusObjectSearch,
}

#[derive(Debug)]
pub enum AppWindowOutput {
    Closed(u64),
}

#[relm4::component(pub)]
impl Component for AppWindow {
    type Init = AppWindowInit;
    type Input = AppWindowMsg;
    type Output = AppWindowOutput;
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
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let app_settings = settings::app_settings();
        let content = WindowContent::builder().launch(()).detach();

        let model = AppWindow {
            id: init.id,
            content,
        };

        let widgets = view_output!();

        let width = app_settings.int("window-width");
        let height = app_settings.int("window-height");

        root.set_default_width(width);
        root.set_default_height(height);
        root.set_application(Some(&relm4::main_adw_application()));

        setup_window_actions(&root, &sender);

        let output_sender = sender.output_sender().clone();
        let id = model.id;

        root.connect_close_request(move |window| {
            let settings = settings::app_settings();
            let _ = settings.set_int("window-width", window.width());
            let _ = settings.set_int("window-height", window.height());
            let _ = output_sender.send(AppWindowOutput::Closed(id));
            glib::Propagation::Proceed
        });

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            AppWindowMsg::OpenConnectionDialog => {
                self.content.emit(WindowContentMsg::OpenConnectionDialog);
            }
            AppWindowMsg::NewQueryTab => {
                self.content.emit(WindowContentMsg::NewQueryTab);
            }
            AppWindowMsg::RunQuery => {
                self.content.emit(WindowContentMsg::RunQuery);
            }
            AppWindowMsg::RefreshTableBrowser => {
                self.content.emit(WindowContentMsg::RefreshActiveBrowseTab);
            }
            AppWindowMsg::FocusEditor => {
                self.content.emit(WindowContentMsg::FocusEditor);
            }
            AppWindowMsg::FocusObjectSearch => {
                self.content.emit(WindowContentMsg::FocusObjectSearch);
            }
        }
    }
}

fn setup_window_actions(root: &adw::ApplicationWindow, sender: &ComponentSender<AppWindow>) {
    let action = gtk::gio::SimpleAction::new("new-connection", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::OpenConnectionDialog);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("new-query-tab", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::NewQueryTab);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("run-query", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::RunQuery);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("refresh-table-browser", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::RefreshTableBrowser);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("focus-editor", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::FocusEditor);
    });
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("search", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::FocusObjectSearch);
    });
    root.add_action(&action);
}
