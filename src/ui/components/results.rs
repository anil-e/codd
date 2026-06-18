use crate::models::query_result::{
    MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT, QueryExecutionResult, QueryResult,
};
use crate::models::result_copy;
use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::ui::components::cell_dialog::show_cell_value_dialog;

pub struct QueryResults {
    status_text: String,
    is_error: bool,
    is_loading: bool,
    result: Option<QueryResult>,
    status_title: String,
    status_description: Option<String>,
    row_limit: usize,
    table_rows: gio::ListStore,
    table_view: gtk::ColumnView,
    rendered_columns: Vec<String>,
    copy_target: Rc<Cell<Option<CopyTarget>>>,
    copy_popover: gtk::PopoverMenu,
}

#[derive(Debug, Clone)]
struct ResultRow {
    index: usize,
    cells: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct CopyTarget {
    row_index: usize,
    column_index: usize,
}

impl CopyTarget {
    fn cell_message(self) -> QueryResultsMsg {
        QueryResultsMsg::CopyCell {
            row_index: self.row_index,
            column_index: self.column_index,
        }
    }

    fn row_message(self) -> QueryResultsMsg {
        QueryResultsMsg::CopyRow(self.row_index)
    }

    fn column_message(self) -> QueryResultsMsg {
        QueryResultsMsg::CopyColumn(self.column_index)
    }
}

#[derive(Debug)]
pub enum QueryResultsMsg {
    Clear,
    Loading,
    Cancelled,
    RowLimitChanged(usize),
    CopyCell {
        row_index: usize,
        column_index: usize,
    },
    CopyRow(usize),
    CopyColumn(usize),
    CopyResults,
    ShowResult(QueryExecutionResult),
    ShowError(String),
}

#[derive(Debug)]
pub enum QueryResultsOutput {
    RowLimitChanged(usize),
    Copied(String),
    ExportCsvRequested,
}

#[relm4::component(pub)]
impl Component for QueryResults {
    type Init = usize;
    type Input = QueryResultsMsg;
    type Output = QueryResultsOutput;
    type CommandOutput = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 0,
            add_css_class: "results-pane",

            #[name = "stack"]
            gtk::Stack {
                set_vexpand: true,

                add_named[Some("status")] = &adw::StatusPage {
                    #[watch]
                    set_icon_name: Some(model.status_icon_name()),
                    #[watch]
                    set_title: &model.status_title,
                    #[watch]
                    set_description: if model.is_error {
                        None
                    } else {
                        model.status_description.as_deref()
                    },

                    #[wrap(Some)]
                    set_child = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 12,
                        set_halign: gtk::Align::Center,

                        gtk::Spinner {
                            #[watch]
                            set_visible: model.is_loading,
                            #[watch]
                            set_spinning: model.is_loading,
                        },

                        gtk::Label {
                            set_selectable: true,
                            set_wrap: true,
                            set_max_width_chars: 90,
                            set_justify: gtk::Justification::Center,
                            #[watch]
                            set_visible: model.is_error,
                            #[watch]
                            set_label: model.status_description.as_deref().unwrap_or_default(),
                        },

                    },
                },

                #[name = "grid"]
                add_named[Some("grid")] = &gtk::ScrolledWindow {
                    set_hexpand: true,
                    set_vexpand: true,
                    set_policy: (gtk::PolicyType::Automatic, gtk::PolicyType::Automatic),
                    set_margin_bottom: 12,
                    add_css_class: "results-table-scroller",
                },
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 8,
                set_halign: gtk::Align::Fill,
                set_hexpand: true,
                set_margin_top: 4,
                set_margin_bottom: 10,
                set_margin_start: 12,
                set_margin_end: 12,
                add_css_class: "results-footer",

                gtk::Box {
                    set_hexpand: true,
                },

                gtk::Spinner {
                    #[watch]
                    set_visible: model.is_loading,
                    #[watch]
                    set_spinning: model.is_loading,
                },

                gtk::Label {
                    set_halign: gtk::Align::Center,
                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                    add_css_class: "caption",
                    add_css_class: "dim-label",
                    #[watch]
                    set_label: &model.status_text,
                    #[watch]
                    set_visible: !model.status_text.is_empty(),
                },

                gtk::Separator {
                    set_orientation: gtk::Orientation::Vertical,
                },

                gtk::Label {
                    set_label: &gettext("Row limit"),
                    add_css_class: "caption",
                    add_css_class: "dim-label",
                },

                gtk::SpinButton {
                    set_range: (
                        MIN_QUERY_RESULT_ROW_LIMIT as f64,
                        MAX_QUERY_RESULT_ROW_LIMIT as f64,
                    ),
                    set_increments: (100.0, 1_000.0),
                    set_numeric: true,
                    set_width_chars: 5,
                    #[watch]
                    set_value: model.row_limit as f64,
                    #[watch]
                    set_sensitive: !model.is_loading,
                    connect_value_changed[sender] => move |spin_button| {
                        sender.input(QueryResultsMsg::RowLimitChanged(
                            spin_button.value_as_int().try_into().unwrap_or_default(),
                        ));
                    },
                },

                gtk::Box {
                    set_hexpand: true,
                },

                gtk::Button {
                    set_tooltip_text: Some(&gettext("Export CSV")),
                    add_css_class: "flat",
                    set_child: Some(&adw::ButtonContent::builder()
                        .icon_name("document-save-symbolic")
                        .label(gettext("Export"))
                        .build()
                    ),
                    #[watch]
                    set_visible: model.result.is_some(),
                    #[watch]
                    set_sensitive: !model.is_loading,
                    connect_clicked[sender] => move |_| {
                        let _ = sender.output(QueryResultsOutput::ExportCsvRequested);
                    },
                },
            },
        }
    }

    fn init(
        row_limit: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let table_rows = gio::ListStore::new::<glib::BoxedAnyObject>();
        let table_view = gtk::ColumnView::new(Some(gtk::NoSelection::new(Some(
            table_rows.clone().upcast::<gio::ListModel>(),
        ))));
        table_view.set_vexpand(true);
        table_view.set_hexpand(true);
        table_view.add_css_class("data-table");
        table_view.add_css_class("query-results-table");
        table_view.set_single_click_activate(false);
        table_view.set_show_row_separators(true);
        table_view.set_show_column_separators(true);

        let copy_target = Rc::new(Cell::new(None));
        let copy_popover = gtk::PopoverMenu::from_model(Some(&copy_menu()));
        copy_popover.set_has_arrow(false);
        copy_popover.set_parent(&root);
        root.insert_action_group(
            "result",
            Some(&copy_action_group(copy_target.clone(), sender.clone())),
        );

        let model = QueryResults {
            status_text: gettext("Run a query to see results"),
            is_error: false,
            is_loading: false,
            result: None,
            status_title: gettext("Run a query"),
            status_description: Some(gettext("Results will appear here after execution.")),
            row_limit,
            table_rows,
            table_view,
            rendered_columns: Vec::new(),
            copy_target,
            copy_popover,
        };
        let widgets = view_output!();
        set_results_stack_child(&widgets, model.result.is_some());
        widgets.grid.set_child(Some(&model.table_view));
        root.set_spacing(0);

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
            QueryResultsMsg::Clear => {
                self.close_copy_menu();
                self.is_loading = false;
                self.is_error = false;
                self.status_text = gettext("Run a query to see results");
                self.result = None;
                self.status_title = gettext("Run a query");
                self.status_description =
                    Some(gettext("Results will appear here after execution."));
            }

            QueryResultsMsg::Loading => {
                self.close_copy_menu();
                self.is_loading = true;
                self.is_error = false;
                self.status_text = gettext("Running query...");
                self.result = None;
                self.status_title = gettext("Running query");
                self.status_description =
                    Some(gettext("Waiting for PostgreSQL to return results."));
            }

            QueryResultsMsg::Cancelled => {
                self.close_copy_menu();
                self.is_loading = false;
                self.is_error = false;
                self.status_text.clear();
                self.result = None;
                self.status_title = gettext("Query cancelled");
                self.status_description = Some(gettext("The query was cancelled."));
            }

            QueryResultsMsg::RowLimitChanged(row_limit) => {
                self.row_limit =
                    row_limit.clamp(MIN_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT);
                let _ = sender.output(QueryResultsOutput::RowLimitChanged(self.row_limit));
                self.update_view(widgets, sender);
                return;
            }

            QueryResultsMsg::CopyCell {
                row_index,
                column_index,
            } => {
                self.copy_text(
                    self.result
                        .as_ref()
                        .and_then(|result| result_copy::cell(result, row_index, column_index)),
                    gettext("Cell copied."),
                    &sender,
                );
                return;
            }

            QueryResultsMsg::CopyRow(row_index) => {
                self.copy_text(
                    self.result
                        .as_ref()
                        .and_then(|result| result_copy::row(result, row_index)),
                    gettext("Row copied."),
                    &sender,
                );
                return;
            }

            QueryResultsMsg::CopyColumn(column_index) => {
                self.copy_text(
                    self.result
                        .as_ref()
                        .and_then(|result| result_copy::column(result, column_index)),
                    gettext("Column copied."),
                    &sender,
                );
                return;
            }

            QueryResultsMsg::CopyResults => {
                self.copy_text(
                    self.result.as_ref().map(result_copy::table),
                    gettext("Results copied."),
                    &sender,
                );
                return;
            }

            QueryResultsMsg::ShowResult(QueryExecutionResult::Rows(result)) => {
                self.close_copy_menu();
                self.is_loading = false;
                self.is_error = false;
                self.status_text = result_status_text(&result);
                self.status_title = if result.rows.is_empty() {
                    gettext("Query returned no rows.")
                } else {
                    String::new()
                };
                self.status_description = None;
                self.result = (!result.rows.is_empty()).then_some(result);
            }

            QueryResultsMsg::ShowResult(QueryExecutionResult::AffectedRows(rows)) => {
                self.close_copy_menu();
                self.is_loading = false;
                self.is_error = false;
                self.status_text.clear();
                let affected_rows = plural_count_u64(rows);
                self.status_title = format!(
                    "{rows} {}",
                    ngettext("row affected", "rows affected", affected_rows)
                );
                self.status_description = Some(gettext("The statement completed successfully."));
                self.result = None;
            }

            QueryResultsMsg::ShowError(error) => {
                self.close_copy_menu();
                self.is_loading = false;
                self.is_error = true;
                self.status_text.clear();
                self.status_title = gettext("Query failed");
                self.status_description = Some(error);
                self.result = None;
            }
        }

        self.render_table();
        set_results_stack_child(widgets, self.result.is_some());
        self.update_view(widgets, sender);
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        self.close_copy_menu();
        if self.copy_popover.parent().is_some() {
            self.copy_popover.unparent();
        }
    }
}

impl QueryResults {
    fn status_icon_name(&self) -> &'static str {
        if self.is_error {
            "dialog-error-symbolic"
        } else if self.is_loading {
            "view-refresh-symbolic"
        } else {
            "network-server-symbolic"
        }
    }

    fn copy_text(
        &self,
        text: Option<String>,
        message: String,
        sender: &ComponentSender<QueryResults>,
    ) {
        let Some(text) = text else {
            return;
        };

        copy_text_to_clipboard(&text);
        let _ = sender.output(QueryResultsOutput::Copied(message));
    }

    fn close_copy_menu(&self) {
        self.copy_popover.popdown();
        self.copy_target.set(None);
    }
}

fn copy_text_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

fn set_results_stack_child(widgets: &QueryResultsWidgets, has_result: bool) {
    widgets
        .stack
        .set_visible_child_name(if has_result { "grid" } else { "status" });
}

fn plural_count(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn plural_count_u64(count: u64) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn result_status_text(result: &QueryResult) -> String {
    if result.rows.is_empty() {
        return gettext("Query returned no rows.");
    }

    let row_suffix = if result.row_limit_reached { "+" } else { "" };
    let row_count = plural_count(result.rows.len());
    format!(
        "{}{row_suffix} {}",
        result.rows.len(),
        ngettext("row", "rows", row_count)
    )
}

impl QueryResults {
    fn render_table(&mut self) {
        let Some(result) = self.result.clone() else {
            self.table_rows.remove_all();
            clear_columns(&self.table_view);
            self.rendered_columns.clear();
            return;
        };

        self.sync_columns(&result);
        self.table_rows.remove_all();
        for (index, row) in result.rows.iter().enumerate() {
            self.table_rows
                .append(&glib::BoxedAnyObject::new(ResultRow {
                    index,
                    cells: row.clone(),
                }));
        }
    }

    fn sync_columns(&mut self, result: &QueryResult) {
        if self.rendered_columns == result.columns {
            return;
        }

        clear_columns(&self.table_view);
        self.rendered_columns.clone_from(&result.columns);

        for (index, column) in result.columns.iter().enumerate() {
            let factory = cell_factory(index, self.copy_popover.clone(), self.copy_target.clone());
            let view_column = gtk::ColumnViewColumn::new(Some(column), Some(factory));
            view_column.set_resizable(true);
            view_column.set_expand(index < 3);
            self.table_view.append_column(&view_column);
        }
    }
}

fn clear_columns(view: &gtk::ColumnView) {
    while let Some(column) = view.columns().item(0) {
        if let Ok(column) = column.downcast::<gtk::ColumnViewColumn>() {
            view.remove_column(&column);
        } else {
            break;
        }
    }
}

fn cell_factory(
    column_index: usize,
    copy_popover: gtk::PopoverMenu,
    copy_target: Rc<Cell<Option<CopyTarget>>>,
) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let list_item = list_item.clone();
        let copy_popover = copy_popover.clone();
        let copy_target = copy_target.clone();

        list_item.set_activatable(false);
        list_item.set_selectable(false);

        let label = gtk::Label::builder()
            .xalign(0.0)
            .focusable(false)
            .selectable(false)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .lines(1)
            .width_chars(12)
            .max_width_chars(28)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();

        label.add_css_class("query-cell");
        label.add_css_class("result-cell");

        label.add_controller({
            let gesture = gtk::GestureClick::new();
            let clicked_item = list_item.clone();
            gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
            gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
            gesture.connect_pressed(move |gesture, _, x, y| {
                let Some(widget) = gesture.widget() else {
                    return;
                };
                let Ok(label) = widget.downcast::<gtk::Label>() else {
                    return;
                };

                gesture.set_state(gtk::EventSequenceState::Claimed);

                if let Some(item) = clicked_item.item()
                    && let Ok(row) = item.downcast::<glib::BoxedAnyObject>()
                {
                    let row = row.borrow::<ResultRow>();
                    copy_target.set(Some(CopyTarget {
                        row_index: row.index,
                        column_index,
                    }));
                    show_copy_menu(&label, &copy_popover, x, y);
                }
            });
            gesture
        });

        label.add_controller({
            let gesture = gtk::GestureClick::new();
            gesture.set_button(gtk::gdk::BUTTON_PRIMARY);
            gesture.connect_pressed(move |gesture, press_count, _, _| {
                if press_count == 2
                    && let Some(widget) = gesture.widget()
                    && let Ok(label) = widget.downcast::<gtk::Label>()
                    && let Some(full_value) = label.tooltip_text()
                {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    show_cell_value_dialog(&label, &full_value);
                }
            });
            gesture
        });

        list_item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(item) = list_item.item() else {
            return;
        };
        let Some(label) = list_item.child().and_downcast::<gtk::Label>() else {
            return;
        };
        let Ok(row) = item.downcast::<glib::BoxedAnyObject>() else {
            return;
        };

        let row = row.borrow::<ResultRow>();
        let value = row.cells.get(column_index).map_or("", String::as_str);
        label.set_label(&display_cell_value(value, false));
        label.set_tooltip_text(Some(value));
    });

    factory
}

fn show_copy_menu(anchor: &gtk::Label, popover: &gtk::PopoverMenu, x: f64, y: f64) {
    if let Some(parent) = popover.parent()
        && let Some(point) =
            anchor.compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
    {
        let rect = gtk::gdk::Rectangle::new(point.x() as i32, point.y() as i32, 1, 1);
        popover.set_pointing_to(Some(&rect));
    }

    popover.popup();
}

fn copy_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some(&gettext("Copy Cell")), Some("result.copy-cell"));
    menu.append(Some(&gettext("Copy Row")), Some("result.copy-row"));
    menu.append(Some(&gettext("Copy Column")), Some("result.copy-column"));
    menu.append(
        Some(&gettext("Copy Displayed Results")),
        Some("result.copy-results"),
    );
    menu.append(Some(&gettext("Export CSV...")), Some("result.export-csv"));

    menu
}

fn copy_action_group(
    copy_target: Rc<Cell<Option<CopyTarget>>>,
    sender: ComponentSender<QueryResults>,
) -> gio::SimpleActionGroup {
    let action_group = gio::SimpleActionGroup::new();
    let actions = [
        "copy-cell",
        "copy-row",
        "copy-column",
        "copy-results",
        "export-csv",
    ];

    for name in actions {
        let simple_action = gio::SimpleAction::new(name, None);
        let sender = sender.clone();
        let copy_target = copy_target.clone();

        simple_action.connect_activate(move |_, _| {
            if name == "export-csv" {
                let _ = sender.output(QueryResultsOutput::ExportCsvRequested);
                return;
            }

            let Some(target) = copy_target.get() else {
                return;
            };

            sender.input(match name {
                "copy-cell" => target.cell_message(),
                "copy-row" => target.row_message(),
                "copy-column" => target.column_message(),
                "copy-results" => QueryResultsMsg::CopyResults,
                _ => return,
            });
        });

        action_group.add_action(&simple_action);
    }

    action_group
}

fn display_cell_value(value: &str, is_header: bool) -> String {
    if is_header || value.chars().count() <= 80 {
        return value.to_string();
    }

    let mut shortened = value.chars().take(80).collect::<String>();
    shortened.push_str("...");
    shortened
}

#[cfg(test)]
mod tests {
    use super::{QueryResult, result_status_text};

    #[test]
    fn result_status_keeps_row_count_when_row_limit_was_reached() {
        let result = QueryResult {
            columns: vec!["id".to_string()],
            rows: vec![vec!["1".to_string()], vec!["2".to_string()]],
            row_limit: Some(2),
            row_limit_reached: true,
        };

        assert_eq!(result_status_text(&result), "2+ rows");
    }

    #[test]
    fn result_status_does_not_mark_unlimited_results() {
        let result = QueryResult {
            columns: vec!["id".to_string()],
            rows: vec![vec!["1".to_string()]],
            row_limit: None,
            row_limit_reached: false,
        };

        assert_eq!(result_status_text(&result), "1 row");
    }
}
