use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::settings;
use crate::window_actions::{self, WindowAction};
use crate::window_content::{WindowContent, WindowContentMsg, WindowContentOutput};

pub struct AppWindowInit {
    pub id: u64,
}

pub struct AppWindow {
    id: u64,
    content: Controller<WindowContent>,
    quitting: bool,
}

#[derive(Debug)]
pub enum AppWindowMsg {
    OpenSqlFiles,
    OpenSqlFileLocations(Vec<gtk::gio::File>),
    SaveSqlFile,
    SaveSqlFileAs,
    RequestClose,
    PrepareClose,
    CancelClosePreparation,
    CommitClose,
    FinishClose,
    ClosePrepared,
    CloseCancelled,
    CloseCommitted,
    SetQuitting(bool),
    OpenConnectionDialog,
    NewQueryTab,
    CloseActiveTab,
    RunQuery,
    CancelQuery,
    RefreshTableBrowser,
    RefreshWorkspace,
    FocusEditor,
    FocusObjectSearch,
}

#[derive(Debug)]
pub enum AppWindowOutput {
    CloseRequested(u64),
    ClosePrepared(u64),
    CloseCancelled(u64),
    CloseCommitted(u64),
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
        let content = WindowContent::builder().launch(()).forward(
            sender.input_sender(),
            |output| match output {
                WindowContentOutput::ClosePrepared => AppWindowMsg::ClosePrepared,
                WindowContentOutput::CloseCancelled => AppWindowMsg::CloseCancelled,
                WindowContentOutput::CloseCommitted => AppWindowMsg::CloseCommitted,
            },
        );

        let model = AppWindow {
            id: init.id,
            content,
            quitting: false,
        };

        let widgets = view_output!();

        let width = app_settings.int("window-width");
        let height = app_settings.int("window-height");

        root.set_default_width(width);
        root.set_default_height(height);
        root.set_application(Some(&relm4::main_adw_application()));

        window_actions::setup_window_actions(&root, {
            let sender = sender.clone();
            move |action| sender.input(AppWindowMsg::from(action))
        });

        root.connect_close_request(move |window| {
            let settings = settings::app_settings();
            let _ = settings.set_int("window-width", window.width());
            let _ = settings.set_int("window-height", window.height());
            sender.input(AppWindowMsg::RequestClose);
            glib::Propagation::Stop
        });

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            AppWindowMsg::OpenSqlFiles if !self.quitting => {
                self.content.emit(WindowContentMsg::OpenSqlFiles)
            }
            AppWindowMsg::OpenSqlFileLocations(files) if !self.quitting => {
                self.content
                    .emit(WindowContentMsg::OpenSqlFileLocations(files));
            }
            AppWindowMsg::SaveSqlFile if !self.quitting => {
                self.content.emit(WindowContentMsg::SaveSqlFile)
            }
            AppWindowMsg::SaveSqlFileAs if !self.quitting => {
                self.content.emit(WindowContentMsg::SaveSqlFileAs)
            }
            AppWindowMsg::OpenSqlFiles
            | AppWindowMsg::OpenSqlFileLocations(_)
            | AppWindowMsg::SaveSqlFile
            | AppWindowMsg::SaveSqlFileAs => {}
            AppWindowMsg::SetQuitting(quitting) => self.quitting = quitting,
            AppWindowMsg::RequestClose => {
                let _ = sender.output(AppWindowOutput::CloseRequested(self.id));
            }
            AppWindowMsg::PrepareClose => self.content.emit(WindowContentMsg::PrepareClose),
            AppWindowMsg::CancelClosePreparation => {
                self.content.emit(WindowContentMsg::CancelClosePreparation);
            }
            AppWindowMsg::CommitClose => self.content.emit(WindowContentMsg::CommitClose),
            AppWindowMsg::ClosePrepared => {
                let _ = sender.output(AppWindowOutput::ClosePrepared(self.id));
            }
            AppWindowMsg::CloseCancelled => {
                let _ = sender.output(AppWindowOutput::CloseCancelled(self.id));
            }
            AppWindowMsg::CloseCommitted => {
                let _ = sender.output(AppWindowOutput::CloseCommitted(self.id));
            }
            AppWindowMsg::FinishClose => {
                root.destroy();
                let _ = sender.output(AppWindowOutput::Closed(self.id));
            }
            AppWindowMsg::OpenConnectionDialog => {
                self.content.emit(WindowContentMsg::OpenConnectionDialog);
            }
            AppWindowMsg::NewQueryTab => {
                self.content.emit(WindowContentMsg::NewQueryTab);
            }
            AppWindowMsg::CloseActiveTab => {
                self.content.emit(WindowContentMsg::CloseActiveTab);
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
            AppWindowMsg::RefreshWorkspace => {
                self.content.emit(WindowContentMsg::RefreshWorkspace);
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

impl From<WindowAction> for AppWindowMsg {
    fn from(action: WindowAction) -> Self {
        match action {
            WindowAction::OpenSqlFiles => Self::OpenSqlFiles,
            WindowAction::SaveSqlFile => Self::SaveSqlFile,
            WindowAction::SaveSqlFileAs => Self::SaveSqlFileAs,
            WindowAction::OpenConnectionDialog => Self::OpenConnectionDialog,
            WindowAction::NewQueryTab => Self::NewQueryTab,
            WindowAction::CloseActiveTab => Self::CloseActiveTab,
            WindowAction::RunQuery => Self::RunQuery,
            WindowAction::CancelQuery => Self::CancelQuery,
            WindowAction::RefreshTableBrowser => Self::RefreshTableBrowser,
            WindowAction::RefreshWorkspace => Self::RefreshWorkspace,
            WindowAction::FocusEditor => Self::FocusEditor,
            WindowAction::FocusObjectSearch => Self::FocusObjectSearch,
        }
    }
}
