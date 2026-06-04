use std::collections::HashMap;

use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::app_window::{AppWindow as ExtraAppWindow, AppWindowInit, AppWindowOutput};
use crate::config::{APP_ID, RESOURCE_PREFIX};
use crate::settings;
use crate::window_actions::{self, WindowAction};
use crate::window_content::{WindowContent, WindowContentMsg};

pub struct AppWindow {
    content: Controller<WindowContent>,
    windows: HashMap<u64, Controller<ExtraAppWindow>>,
    next_window_id: u64,
}

#[derive(Debug)]
pub enum AppWindowMsg {
    OpenConnectionDialog,
    NewQueryTab,
    RunQuery,
    CancelQuery,
    RefreshTableBrowser,
    FocusEditor,
    FocusObjectSearch,
    NewWindow,
    ExtraWindowClosed(u64),
    Shortcuts,
    About,
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
        let model = AppWindow {
            content,
            windows: HashMap::new(),
            next_window_id: 1,
        };
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

        window_actions::setup_window_actions(&root, {
            let sender = sender.clone();
            move |action| sender.input(AppWindowMsg::from(action))
        });
        setup_app_actions(&sender);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
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
            AppWindowMsg::CancelQuery => {
                self.content.emit(WindowContentMsg::CancelQuery);
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
            AppWindowMsg::NewWindow => {
                self.spawn_window(&sender);
            }
            AppWindowMsg::ExtraWindowClosed(id) => {
                self.windows.remove(&id);
            }
            AppWindowMsg::Shortcuts => {
                show_shortcuts_dialog(root);
            }
            AppWindowMsg::About => {
                show_about_dialog(root);
            }
            AppWindowMsg::Quit => {
                relm4::main_adw_application().quit();
            }
        }
    }
}

fn setup_app_actions(sender: &ComponentSender<AppWindow>) {
    let app = relm4::main_adw_application();

    let action = gtk::gio::SimpleAction::new("new-window", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::NewWindow);
    });
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("shortcuts", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::Shortcuts);
    });
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("about", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::About);
    });
    app.add_action(&action);

    let action = gtk::gio::SimpleAction::new("quit", None);
    let s = sender.clone();
    action.connect_activate(move |_, _| {
        s.input(AppWindowMsg::Quit);
    });
    app.add_action(&action);

    app.set_accels_for_action("win.new-query-tab", &["<Control>n"]);
    app.set_accels_for_action("app.new-window", &["<Control><Shift>n"]);
    app.set_accels_for_action("win.run-query", &["<Control>Return"]);
    app.set_accels_for_action("win.cancel-query", &["Escape"]);
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

    let app = relm4::main_adw_application();
    if let Some(window) = app.active_window() {
        dialog.present(Some(&window));
    } else {
        dialog.present(Some(root));
    }
}

fn show_about_dialog(root: &adw::ApplicationWindow) {
    let about = adw::AboutDialog::builder()
        .application_name("Codd")
        .application_icon(APP_ID)
        .developer_name("Anil Erdogan")
        .version(env!("CARGO_PKG_VERSION"))
        .developers(vec!["Anil Erdogan"])
        .translator_credits(gettext("translator-credits"))
        .copyright("© 2026 Anil Erdogan")
        .comments(gettext("Lightweight PostgreSQL client"))
        .website("https://github.com/anil-e/codd")
        .issue_url("https://github.com/anil-e/codd/issues")
        .license_type(gtk::License::Agpl30)
        .build();

    let app = relm4::main_adw_application();
    if let Some(window) = app.active_window() {
        about.present(Some(&window));
    } else {
        about.present(Some(root));
    }
}

impl From<WindowAction> for AppWindowMsg {
    fn from(action: WindowAction) -> Self {
        match action {
            WindowAction::OpenConnectionDialog => Self::OpenConnectionDialog,
            WindowAction::NewQueryTab => Self::NewQueryTab,
            WindowAction::RunQuery => Self::RunQuery,
            WindowAction::CancelQuery => Self::CancelQuery,
            WindowAction::RefreshTableBrowser => Self::RefreshTableBrowser,
            WindowAction::FocusEditor => Self::FocusEditor,
            WindowAction::FocusObjectSearch => Self::FocusObjectSearch,
        }
    }
}

impl AppWindow {
    fn spawn_window(&mut self, sender: &ComponentSender<Self>) {
        let id = self.next_window_id;
        self.next_window_id = self.next_window_id.wrapping_add(1);

        let window = ExtraAppWindow::builder()
            .launch(AppWindowInit { id })
            .forward(sender.input_sender(), |output| match output {
                AppWindowOutput::Closed(id) => AppWindowMsg::ExtraWindowClosed(id),
            });

        self.windows.insert(id, window);
    }
}
