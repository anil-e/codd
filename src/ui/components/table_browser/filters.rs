use std::cell::RefCell;
use std::rc::Rc;

use gettextrs::gettext;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::models::table_browser::{ColumnTypeGroup, FilterOperator, TableColumn, TableFilter};
use crate::ui::components::table_browser::{TableBrowser, TableBrowserMsg};

const FILTER_FIELD_COLUMNS: i32 = 3;
const FILTER_OPERATOR_COLUMNS: i32 = 1;
const FILTER_VALUE_COLUMNS: i32 = 3;

#[derive(Debug)]
pub(crate) enum FilterEvent {
    DraftChanged(Vec<TableFilter>),
    DraftValuesChanged(Vec<TableFilter>),
    Apply(Vec<TableFilter>),
    Clear,
}

pub(super) struct FilterPanel;

pub(super) fn initial_filter(columns: &[TableColumn]) -> Option<TableFilter> {
    let column = columns.first()?;
    let operator = FilterOperator::for_column(column)[0];

    Some(TableFilter::column(
        column.name.clone(),
        operator,
        default_filter_value(column, operator),
    ))
}

impl FilterPanel {
    pub(super) fn rebuild(
        container: &gtk::Box,
        columns: Option<&[TableColumn]>,
        filters: &[TableFilter],
        has_active_filters: bool,
        sender: &ComponentSender<TableBrowser>,
    ) {
        clear_box(container);
        container.set_margin_top(0);
        container.set_margin_bottom(8);
        container.set_margin_start(12);
        container.set_margin_end(12);

        let Some(columns) = columns else {
            append_empty_label(container, &gettext("Load the table before adding filters."));
            return;
        };

        if columns.is_empty() {
            append_empty_label(container, &gettext("This table has no filterable columns."));
            return;
        }

        let normalized = normalize_filters(columns, filters);
        let state = Rc::new(RefCell::new(normalized.clone()));
        if normalized.is_empty() {
            append_empty_label(container, &gettext("No filters applied."));
        } else {
            for (index, filter) in normalized.iter().enumerate() {
                container.append(&filter_row(index, columns, filter, &state, sender));
            }
        }

        container.append(&actions_row(
            columns,
            &normalized,
            has_active_filters,
            &state,
            sender,
        ));
        container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }
}

pub(super) fn validate_filter_values(filters: &[TableFilter]) -> Result<(), String> {
    for filter in filters {
        match filter {
            TableFilter::CustomSql { expression } if expression.trim().is_empty() => {
                return Err(gettext("Missing custom SQL filter"));
            }

            TableFilter::CustomSql { .. } => {}

            TableFilter::Column {
                column_name,
                operator,
                value,
            } if operator.needs_value()
                && value.as_ref().is_none_or(|value| value.trim().is_empty()) =>
            {
                return Err(format!(
                    "{}: {}",
                    gettext("Missing value for filter"),
                    column_name
                ));
            }

            TableFilter::Column { .. } => {}
        }
    }

    Ok(())
}

fn filter_row(
    index: usize,
    columns: &[TableColumn],
    filter: &TableFilter,
    state: &Rc<RefCell<Vec<TableFilter>>>,
    sender: &ComponentSender<TableBrowser>,
) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .hexpand(true)
        .build();
    let fields = gtk::Grid::builder()
        .column_homogeneous(true)
        .column_spacing(8)
        .hexpand(true)
        .build();

    let column_dropdown = gtk::DropDown::builder()
        .model(&filter_field_model(columns))
        .selected(filter_field_index(columns, filter) as u32)
        .hexpand(true)
        .build();

    column_dropdown.add_css_class("compact");

    fields.attach(&column_dropdown, 0, 0, FILTER_FIELD_COLUMNS, 1);

    if let TableFilter::CustomSql { expression } = filter {
        fields.attach(
            &custom_sql_editor(index, expression, state, sender),
            FILTER_FIELD_COLUMNS,
            0,
            FILTER_OPERATOR_COLUMNS + FILTER_VALUE_COLUMNS,
            1,
        );
    } else if let TableFilter::Column {
        operator, value, ..
    } = filter
    {
        let column = filter_column(columns, filter).unwrap_or(&columns[0]);
        let operators = FilterOperator::for_column(column);

        let operator_dropdown = gtk::DropDown::builder()
            .model(&operator_model(operators))
            .selected(operator_index(operators, *operator) as u32)
            .hexpand(true)
            .build();

        operator_dropdown.add_css_class("compact");

        fields.attach(
            &operator_dropdown,
            FILTER_FIELD_COLUMNS,
            0,
            FILTER_OPERATOR_COLUMNS,
            1,
        );

        if operator.needs_value() {
            fields.attach(
                &value_editor(index, columns, filter, value.as_deref(), state, sender),
                FILTER_FIELD_COLUMNS + FILTER_OPERATOR_COLUMNS,
                0,
                FILTER_VALUE_COLUMNS,
                1,
            );
        } else {
            fields.attach(
                &empty_value_slot(),
                FILTER_FIELD_COLUMNS + FILTER_OPERATOR_COLUMNS,
                0,
                FILTER_VALUE_COLUMNS,
                1,
            );
        }

        operator_dropdown.connect_selected_notify({
            let sender = sender.clone();
            let columns = columns.to_vec();
            let state = state.clone();

            move |dropdown| {
                let mut updated = state.borrow().clone();

                let Some(filter @ TableFilter::Column { .. }) = updated.get_mut(index) else {
                    return;
                };

                let Some(column) = filter_column(&columns, filter) else {
                    return;
                };

                let operators = FilterOperator::for_column(column);

                let Some(operator) = operators.get(dropdown.selected() as usize).copied() else {
                    return;
                };

                if let TableFilter::Column {
                    operator: filter_operator,
                    value,
                    ..
                } = filter
                {
                    *filter_operator = operator;
                    if operator.needs_value() {
                        *value = value
                            .take()
                            .filter(|value| !value.is_empty())
                            .or_else(|| default_filter_value(column, operator));
                    } else {
                        *value = None;
                    }
                }

                sender.input(TableBrowserMsg::FilterEvent(FilterEvent::DraftChanged(
                    updated,
                )));
            }
        });
    }

    row.append(&fields);

    let remove_button = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text(gettext("Remove filter"))
        .build();
    remove_button.add_css_class("flat");
    row.append(&remove_button);

    column_dropdown.connect_selected_notify({
        let sender = sender.clone();
        let columns = columns.to_vec();
        let state = state.clone();

        move |dropdown| {
            let mut updated = state.borrow().clone();
            if let Some(filter) = updated.get_mut(index) {
                if dropdown.selected() as usize == columns.len() {
                    *filter = TableFilter::custom_sql("");
                } else if let Some(column) = columns.get(dropdown.selected() as usize) {
                    let operator = FilterOperator::for_column(column)[0];
                    *filter = TableFilter::column(
                        column.name.clone(),
                        operator,
                        default_filter_value(column, operator),
                    );
                }
            }

            sender.input(TableBrowserMsg::FilterEvent(FilterEvent::DraftChanged(
                updated,
            )));
        }
    });

    remove_button.connect_clicked({
        let sender = sender.clone();
        let state = state.clone();

        move |_| {
            let mut updated = state.borrow().clone();
            if index < updated.len() {
                updated.remove(index);
            }

            sender.input(TableBrowserMsg::FilterEvent(FilterEvent::DraftChanged(
                updated,
            )));
        }
    });

    row
}

fn value_editor(
    index: usize,
    columns: &[TableColumn],
    filter: &TableFilter,
    value: Option<&str>,
    state: &Rc<RefCell<Vec<TableFilter>>>,
    sender: &ComponentSender<TableBrowser>,
) -> gtk::Widget {
    let column = filter_column(columns, filter).unwrap_or(&columns[0]);

    if column.type_group == ColumnTypeGroup::Boolean {
        return choice_editor(index, &["false", "true"], value, state, sender);
    }

    if !column.enum_values.is_empty() {
        let values = column
            .enum_values
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        return choice_editor(index, &values, value, state, sender);
    }

    let entry = gtk::Entry::builder()
        .text(value.unwrap_or_default())
        .placeholder_text(gettext("Value"))
        .hexpand(true)
        .build();

    entry.connect_changed({
        let sender = sender.clone();
        let state = state.clone();

        move |entry| {
            let updated = update_state_value(&state, index, Some(entry.text().to_string()));

            sender.input(TableBrowserMsg::FilterEvent(
                FilterEvent::DraftValuesChanged(updated),
            ));
        }
    });

    entry.upcast()
}

fn custom_sql_editor(
    index: usize,
    expression: &str,
    state: &Rc<RefCell<Vec<TableFilter>>>,
    sender: &ComponentSender<TableBrowser>,
) -> gtk::Widget {
    let entry = gtk::Entry::builder()
        .text(expression)
        .placeholder_text(gettext("SQL expression"))
        .hexpand(true)
        .build();

    entry.connect_changed({
        let sender = sender.clone();
        let state = state.clone();

        move |entry| {
            let updated = update_state_value(&state, index, Some(entry.text().to_string()));

            sender.input(TableBrowserMsg::FilterEvent(
                FilterEvent::DraftValuesChanged(updated),
            ));
        }
    });

    entry.upcast()
}

fn choice_editor(
    index: usize,
    values: &[&str],
    value: Option<&str>,
    state: &Rc<RefCell<Vec<TableFilter>>>,
    sender: &ComponentSender<TableBrowser>,
) -> gtk::Widget {
    let dropdown = gtk::DropDown::builder()
        .model(&gtk::StringList::new(values))
        .selected(choice_index(values, value) as u32)
        .hexpand(true)
        .build();
    dropdown.add_css_class("compact");

    dropdown.connect_selected_notify({
        let sender = sender.clone();
        let state = state.clone();
        let values = values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();

        move |dropdown| {
            let value = values.get(dropdown.selected() as usize).cloned();
            let updated = update_state_value(&state, index, value);

            sender.input(TableBrowserMsg::FilterEvent(
                FilterEvent::DraftValuesChanged(updated),
            ));
        }
    });

    dropdown.upcast()
}

fn empty_value_slot() -> gtk::Widget {
    gtk::Box::builder().hexpand(true).build().upcast()
}

fn actions_row(
    columns: &[TableColumn],
    filters: &[TableFilter],
    has_active_filters: bool,
    state: &Rc<RefCell<Vec<TableFilter>>>,
    sender: &ComponentSender<TableBrowser>,
) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::End)
        .build();

    let add_button = gtk::Button::with_label(&gettext("Add Filter"));
    add_button.add_css_class("flat");

    let clear_button = gtk::Button::with_label(&gettext("Clear"));
    clear_button.add_css_class("flat");
    clear_button.set_sensitive(!filters.is_empty() || has_active_filters);

    let apply_button = gtk::Button::with_label(&gettext("Apply"));
    apply_button.add_css_class("suggested-action");
    apply_button.set_sensitive(!filters.is_empty() || has_active_filters);

    row.append(&add_button);
    row.append(&clear_button);
    row.append(&apply_button);

    add_button.connect_clicked({
        let sender = sender.clone();
        let columns = columns.to_vec();
        let state = state.clone();

        move |_| {
            let mut updated = state.borrow().clone();
            if let Some(filter) = initial_filter(&columns) {
                updated.push(filter);
            }

            sender.input(TableBrowserMsg::FilterEvent(FilterEvent::DraftChanged(
                updated,
            )));
        }
    });

    clear_button.connect_clicked({
        let sender = sender.clone();

        move |_| {
            sender.input(TableBrowserMsg::FilterEvent(FilterEvent::Clear));
        }
    });

    apply_button.connect_clicked({
        let sender = sender.clone();
        let state = state.clone();

        move |_| {
            sender.input(TableBrowserMsg::FilterEvent(FilterEvent::Apply(
                state.borrow().clone(),
            )));
        }
    });

    row
}

fn normalize_filters(columns: &[TableColumn], filters: &[TableFilter]) -> Vec<TableFilter> {
    filters
        .iter()
        .filter_map(|filter| {
            let TableFilter::Column {
                operator, value, ..
            } = filter
            else {
                return Some(filter.clone());
            };

            let column = filter_column(columns, filter)?;
            let operator = if operator.is_supported_for(column) {
                *operator
            } else {
                FilterOperator::for_column(column)[0]
            };

            Some(TableFilter::column(
                column.name.clone(),
                operator,
                if operator.needs_value() {
                    value
                        .clone()
                        .or_else(|| default_filter_value(column, operator))
                } else {
                    None
                },
            ))
        })
        .collect()
}

fn update_state_value(
    state: &Rc<RefCell<Vec<TableFilter>>>,
    index: usize,
    value: Option<String>,
) -> Vec<TableFilter> {
    let mut filters = state.borrow_mut();

    if let Some(filter) = filters.get_mut(index) {
        match filter {
            TableFilter::Column {
                value: filter_value,
                ..
            } => *filter_value = value,
            TableFilter::CustomSql { expression } => *expression = value.unwrap_or_default(),
        }
    }

    filters.clone()
}

fn default_filter_value(column: &TableColumn, operator: FilterOperator) -> Option<String> {
    if !operator.needs_value() {
        return None;
    }

    if column.type_group == ColumnTypeGroup::Boolean {
        return Some("true".to_string());
    }

    column.enum_values.first().cloned()
}

fn filter_column<'a>(columns: &'a [TableColumn], filter: &TableFilter) -> Option<&'a TableColumn> {
    let TableFilter::Column { column_name, .. } = filter else {
        return None;
    };

    columns.iter().find(|column| column.name == *column_name)
}

fn filter_field_index(columns: &[TableColumn], filter: &TableFilter) -> usize {
    let TableFilter::Column { column_name, .. } = filter else {
        return columns.len();
    };

    columns
        .iter()
        .position(|column| column.name == *column_name)
        .unwrap_or(0)
}

fn operator_index(operators: &[FilterOperator], operator: FilterOperator) -> usize {
    operators
        .iter()
        .position(|candidate| *candidate == operator)
        .unwrap_or(0)
}

fn choice_index(values: &[&str], value: Option<&str>) -> usize {
    values
        .iter()
        .position(|candidate| Some(*candidate) == value)
        .unwrap_or(0)
}

fn operator_model(operators: &[FilterOperator]) -> gtk::StringList {
    let labels = operators
        .iter()
        .map(|operator| operator.label())
        .collect::<Vec<_>>();

    gtk::StringList::new(&labels)
}

fn filter_field_model(columns: &[TableColumn]) -> gtk::StringList {
    let mut values = columns
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    values.push(gettext("Custom SQL"));

    string_list(&values)
}

fn string_list(values: &[String]) -> gtk::StringList {
    let borrowed = values.iter().map(String::as_str).collect::<Vec<_>>();

    gtk::StringList::new(&borrowed)
}

fn append_empty_label(container: &gtk::Box, text: &str) {
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .build();

    label.add_css_class("caption");
    label.add_css_class("dim-label");
    container.append(&label);
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
