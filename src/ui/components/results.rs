use crate::models::query_result::{
    MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT, QueryExecutionResult, QueryResult,
};
use gettextrs::{gettext, ngettext};
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::{gio, glib};
use relm4::prelude::*;

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
}

#[derive(Debug)]
pub enum QueryResultsMsg {
    Clear,
    Loading,
    Cancelled,
    RowLimitChanged(usize),
    ShowResult(QueryExecutionResult),
    ShowError(String),
}

#[derive(Debug)]
pub enum QueryResultsOutput {
    RowLimitChanged(usize),
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
                    set_description: model.status_description.as_deref(),

                    #[wrap(Some)]
                    set_child = &gtk::Spinner {
                        #[watch]
                        set_visible: model.is_loading,
                        #[watch]
                        set_spinning: model.is_loading,
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
                set_halign: gtk::Align::Center,
                set_margin_top: 4,
                set_margin_bottom: 10,
                set_margin_start: 12,
                set_margin_end: 12,
                add_css_class: "results-footer",

                gtk::Spinner {
                    #[watch]
                    set_visible: model.is_loading,
                    #[watch]
                    set_spinning: model.is_loading,
                },

                gtk::Label {
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
        table_view.set_show_row_separators(true);
        table_view.set_show_column_separators(true);

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
                self.is_loading = false;
                self.is_error = false;
                self.status_text = gettext("Run a query to see results");
                self.result = None;
                self.status_title = gettext("Run a query");
                self.status_description =
                    Some(gettext("Results will appear here after execution."));
            }

            QueryResultsMsg::Loading => {
                self.is_loading = true;
                self.is_error = false;
                self.status_text = gettext("Running query...");
                self.result = None;
                self.status_title = gettext("Running query");
                self.status_description =
                    Some(gettext("Waiting for PostgreSQL to return results."));
            }

            QueryResultsMsg::Cancelled => {
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

            QueryResultsMsg::ShowResult(QueryExecutionResult::Rows(result)) => {
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
                self.is_loading = false;
                self.is_error = true;
                self.status_text.clear();
                self.status_title = gettext("Query failed");
                self.status_description = Some(error);
                self.result = None;
            }
        }

        self.render_table(widgets);
        set_results_stack_child(widgets, self.result.is_some());
        self.update_view(widgets, sender);
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
    fn render_table(&mut self, widgets: &mut QueryResultsWidgets) {
        let Some(result) = self.result.clone() else {
            self.table_rows.remove_all();
            clear_columns(&self.table_view);
            self.rendered_columns.clear();
            return;
        };

        self.sync_columns(&result, widgets);
        self.table_rows.remove_all();
        for row in &result.rows {
            self.table_rows
                .append(&glib::BoxedAnyObject::new(row.clone()));
        }
    }

    fn sync_columns(&mut self, result: &QueryResult, widgets: &QueryResultsWidgets) {
        if self.rendered_columns == result.columns {
            return;
        }

        clear_columns(&self.table_view);
        self.rendered_columns.clone_from(&result.columns);
        widgets.grid.set_min_content_width(480);

        for (index, column) in result.columns.iter().enumerate() {
            let factory = cell_factory(index);
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

fn cell_factory(column_index: usize) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };

        let label = gtk::Label::builder()
            .xalign(0.0)
            .selectable(true)
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

        label.add_controller({
            let gesture = gtk::GestureClick::new();
            gesture.connect_pressed(move |gesture, press_count, _, _| {
                if press_count == 2
                    && let Some(widget) = gesture.widget()
                    && let Ok(label) = widget.downcast::<gtk::Label>()
                    && let Some(full_value) = label.tooltip_text()
                {
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

        let row = row.borrow::<Vec<String>>();
        let value = row.get(column_index).map_or("", String::as_str);
        label.set_label(&display_cell_value(value, false));
        label.set_tooltip_text(Some(value));
    });

    factory
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
}
