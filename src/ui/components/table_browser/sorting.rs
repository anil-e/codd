use libadwaita::prelude::*;
use relm4::gtk;
use relm4::prelude::*;

use crate::models::table_browser::{SortDirection, TableSort};
use crate::ui::components::table_browser::{TableBrowser, TableBrowserMsg};

pub(super) fn next_sort_for_header_click(
    current: Option<&TableSort>,
    clicked: TableSort,
) -> Option<TableSort> {
    let is_third_click = current.is_some_and(|current| {
        current.column_name == clicked.column_name
            && current.direction == SortDirection::Descending
            && clicked.direction == SortDirection::Ascending
    });

    (!is_third_click).then_some(clicked)
}

pub(super) fn sync_sort_indicator(view: &gtk::ColumnView, sort: Option<&TableSort>) {
    let Some(sort) = sort else {
        view.sort_by_column(None, gtk::SortType::Ascending);
        return;
    };

    let Some(column) = column_view_column_by_title(view, &sort.column_name) else {
        view.sort_by_column(None, gtk::SortType::Ascending);
        return;
    };

    view.sort_by_column(
        Some(&column),
        match sort.direction {
            SortDirection::Ascending => gtk::SortType::Ascending,
            SortDirection::Descending => gtk::SortType::Descending,
        },
    );
}

pub(super) fn connect_sort_handlers(
    view: &gtk::ColumnView,
    sender: &ComponentSender<TableBrowser>,
) {
    let Some(sorter) = view.sorter() else {
        return;
    };

    sorter.connect_changed({
        let sender = sender.clone();

        move |sorter, _| {
            if let Some(sort) = sort_from_sorter(sorter) {
                sender.input(TableBrowserMsg::SortChanged(sort));
            }
        }
    });
}

fn sort_from_sorter(sorter: &gtk::Sorter) -> Option<TableSort> {
    let column = sorter.property::<Option<gtk::ColumnViewColumn>>("primary-sort-column")?;
    let column_name = column.title()?.to_string();
    let direction = match sorter.property::<gtk::SortType>("primary-sort-order") {
        gtk::SortType::Ascending => SortDirection::Ascending,
        gtk::SortType::Descending => SortDirection::Descending,
        _ => return None,
    };

    Some(TableSort::new(column_name, direction))
}

fn column_view_column_by_title(
    view: &gtk::ColumnView,
    title: &str,
) -> Option<gtk::ColumnViewColumn> {
    for index in 0..view.columns().n_items() {
        let Some(column) = view
            .columns()
            .item(index)
            .and_downcast::<gtk::ColumnViewColumn>()
        else {
            continue;
        };

        if column.title().as_deref() == Some(title) {
            return Some(column);
        }
    }

    None
}
