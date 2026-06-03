use crate::models::csv_export::{CsvDelimiter, CsvExportOptions};
use crate::models::query_result::{MAX_QUERY_RESULT_ROW_LIMIT, MIN_QUERY_RESULT_ROW_LIMIT};
use gettextrs::gettext;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::gtk;
use relm4::gtk::gio;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub fn show_csv_export_options_dialog(
    parent: Option<&gtk::Window>,
    on_confirm: impl FnOnce(CsvExportOptions) + 'static,
) {
    let delimiter_model =
        gtk::StringList::new(&[&gettext("Comma"), &gettext("Semicolon"), &gettext("Tab")]);
    let delimiter_row = adw::ComboRow::builder()
        .title(gettext("Delimiter"))
        .model(&delimiter_model)
        .selected(0)
        .build();

    let row_limit_spin = gtk::SpinButton::with_range(
        MIN_QUERY_RESULT_ROW_LIMIT as f64,
        MAX_QUERY_RESULT_ROW_LIMIT as f64,
        100.0,
    );

    row_limit_spin.set_value(CsvExportOptions::default().row_limit as f64);
    row_limit_spin.set_numeric(true);
    row_limit_spin.set_width_chars(6);
    row_limit_spin.set_valign(gtk::Align::Center);

    let row_limit_row = adw::ActionRow::builder()
        .title(gettext("Row limit"))
        .build();

    row_limit_row.add_suffix(&row_limit_spin);
    row_limit_row.set_activatable_widget(Some(&row_limit_spin));

    let group = adw::PreferencesGroup::builder().margin_top(12).build();
    group.add(&delimiter_row);
    group.add(&row_limit_row);

    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Export CSV"))
        .body(gettext(
            "Choose how many rows to export and which delimiter to use.",
        ))
        .extra_child(&group)
        .build();

    dialog.add_responses(&[
        ("cancel", &gettext("Cancel")),
        ("export", &gettext("Export")),
    ]);

    dialog.set_default_response(Some("export"));
    dialog.set_response_appearance("export", adw::ResponseAppearance::Suggested);

    let mut on_confirm = Some(on_confirm);

    dialog.choose(parent, None::<&gio::Cancellable>, move |response| {
        if response != "export" {
            return;
        }

        let Some(on_confirm) = on_confirm.take() else {
            return;
        };

        on_confirm(CsvExportOptions {
            delimiter: delimiter_from_index(delimiter_row.selected()),
            row_limit: (row_limit_spin.value_as_int() as usize)
                .clamp(MIN_QUERY_RESULT_ROW_LIMIT, MAX_QUERY_RESULT_ROW_LIMIT),
        });
    });
}

pub fn show_csv_save_dialog(
    parent: Option<&gtk::Window>,
    initial_name: String,
    on_selected: impl FnOnce(PathBuf) + 'static,
) {
    let dialog = gtk::FileChooserNative::new(
        Some(&gettext("Save CSV")),
        parent,
        gtk::FileChooserAction::Save,
        Some(&gettext("Save")),
        Some(&gettext("Cancel")),
    );

    dialog.set_modal(true);
    dialog.set_current_name(&initial_name);

    let csv_filter = gtk::FileFilter::new();
    csv_filter.set_name(Some(&gettext("CSV Files")));
    csv_filter.add_pattern("*.csv");

    dialog.add_filter(&csv_filter);
    dialog.set_filter(&csv_filter);

    let on_selected = Rc::new(RefCell::new(Some(on_selected)));
    dialog.connect_response(move |dialog, response| {
        if response != gtk::ResponseType::Accept {
            return;
        }

        let Some(mut path) = dialog.file().and_then(|file| file.path()) else {
            return;
        };

        if path.extension().is_none() {
            path.set_extension("csv");
        }

        if let Some(on_selected) = on_selected.borrow_mut().take() {
            on_selected(path);
        }
    });

    dialog.show();
}

fn delimiter_from_index(index: u32) -> CsvDelimiter {
    match index {
        1 => CsvDelimiter::Semicolon,
        2 => CsvDelimiter::Tab,
        _ => CsvDelimiter::Comma,
    }
}
