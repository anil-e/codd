# Codd

Codd is a lightweight native PostgreSQL client for GNOME, built with Rust, Relm4, GTK4, libadwaita, GtkSourceView and sqlx. It is named after Edgar F. Codd, the computer scientist who introduced the relational model for database management.

![Codd screenshot](images/codd-screenshot-05-05-2.png)

The app started as a personal-use SQL editor: lightweight, native, focused, and built around clean PostgreSQL query workflows without Electron. It can save multiple PostgreSQL connections, but it is not trying to be a multi-engine database suite. It is still early and many extensions are planned, including richer result handling, editor actions, and more production-quality database tooling.

## Features

- Save and reopen PostgreSQL connections
- Browse schemas, tables, and views
- Write SQL with syntax highlighting and line numbers
- Execute statements and cancel running queries
- Reuse automatically saved query history per connection
- Inspect result sets in a native GTK table
- Keep basic window and sidebar state between sessions

## Requirements

Fedora:

```bash
sudo dnf install \
  rust cargo meson ninja-build pkgconf-pkg-config \
  gtk4-devel libadwaita-devel gtksourceview5-devel \
  glib2-devel glib2-devel-tools \
  desktop-file-utils appstream gettext
```

Arch:

```bash
sudo pacman -S \
  rust cargo meson ninja pkgconf \
  gtk4 libadwaita gtksourceview5 \
  glib2 desktop-file-utils appstream gettext
```

## Run

```bash
cargo run
```

## Build & Install

```bash
meson setup _build
meson compile -C _build
sudo meson install -C _build
```

After installation, launch Codd from your app menu or run `codd`.

## Demo Database

A small PostgreSQL demo database is available in `dev/demo_database.sql`.

```bash
psql -f dev/demo_database.sql
```
