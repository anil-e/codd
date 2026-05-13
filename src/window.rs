use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::config::RESOURCE_PREFIX;
use crate::settings;
use crate::window_content::{WindowContent, WindowContentMsg};

pub struct AppWindow {
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
    Shortcuts,
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

        relm4::main_adw_application().set_resource_base_path(Some(RESOURCE_PREFIX));

        setup_window_actions(&root, &sender);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>, root: &Self::Root) {
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
            AppWindowMsg::Shortcuts => {
                show_shortcuts_dialog(root);
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
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::OpenConnectionDialog);
        }
    ));
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("new-query-tab", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::NewQueryTab);
        }
    ));
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("run-query", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::RunQuery);
        }
    ));
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("refresh-table-browser", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::RefreshTableBrowser);
        }
    ));
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("focus-editor", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::FocusEditor);
        }
    ));
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("search", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::FocusObjectSearch);
        }
    ));
    root.add_action(&action);

    let action = gtk::gio::SimpleAction::new("shortcuts", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::Shortcuts);
        }
    ));
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("quit", None);
    action.connect_activate(glib::clone!(
        #[strong(rename_to=s)]
        sender,
        move |_, _| {
            s.input(AppWindowMsg::Quit);
        }
    ));
    app.add_action(&action);

    app.set_accels_for_action("win.new-query-tab", &["<Control>n"]);
    app.set_accels_for_action("win.new-connection", &["<Control><Shift>n"]);
    app.set_accels_for_action("win.run-query", &["<Control>Return"]);
    app.set_accels_for_action("win.refresh-table-browser", &["<Control>r"]);
    app.set_accels_for_action("win.focus-editor", &["<Control>e"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("app.shortcuts", &["<Control>question"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}

fn show_shortcuts_dialog(root: &adw::ApplicationWindow) {
    let resource_path = format!("{RESOURCE_PREFIX}/ui/shortcuts-dialog.ui");
    let builder = gtk::Builder::from_resource(&resource_path);
    let dialog: adw::ShortcutsDialog = builder
        .object("shortcuts_dialog")
        .expect("shortcuts dialog to exist");

    dialog.present(Some(root));
}
