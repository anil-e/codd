use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
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

pub fn setup_window_actions(
    root: &adw::ApplicationWindow,
    emit: impl Fn(WindowAction) + Clone + 'static,
) {
    add_window_action(
        root,
        "new-connection",
        WindowAction::OpenConnectionDialog,
        emit.clone(),
    );
    add_window_action(
        root,
        "new-query-tab",
        WindowAction::NewQueryTab,
        emit.clone(),
    );
    add_window_action(
        root,
        "close-active-tab",
        WindowAction::CloseActiveTab,
        emit.clone(),
    );
    add_window_action(root, "run-query", WindowAction::RunQuery, emit.clone());
    add_window_action(
        root,
        "cancel-query",
        WindowAction::CancelQuery,
        emit.clone(),
    );
    add_window_action(
        root,
        "refresh-table-browser",
        WindowAction::RefreshTableBrowser,
        emit.clone(),
    );
    add_window_action(
        root,
        "refresh-workspace",
        WindowAction::RefreshWorkspace,
        emit.clone(),
    );
    add_window_action(
        root,
        "focus-editor",
        WindowAction::FocusEditor,
        emit.clone(),
    );
    add_window_action(root, "search", WindowAction::FocusObjectSearch, emit);
}

fn add_window_action(
    root: &adw::ApplicationWindow,
    name: &str,
    action: WindowAction,
    emit: impl Fn(WindowAction) + 'static,
) {
    let simple_action = gtk::gio::SimpleAction::new(name, None);
    simple_action.connect_activate(move |_, _| {
        emit(action);
    });
    root.add_action(&simple_action);
}
