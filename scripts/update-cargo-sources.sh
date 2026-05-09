#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
GENERATOR="$SCRIPT_DIR/flatpak-cargo-generator.py"

if [ ! -f "$GENERATOR" ]; then
    echo "Downloading flatpak-cargo-generator.py..."
    curl -sL -o "$GENERATOR" \
        https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
fi

echo "Generating cargo-sources.json..."
python3 "$GENERATOR" "$PROJECT_DIR/Cargo.lock" -o "$PROJECT_DIR/cargo-sources.json"

# Rename deprecated "config" to "config.toml" in generated sources.
sed -i 's/"dest-filename": "config"/"dest-filename": "config.toml"/' "$PROJECT_DIR/cargo-sources.json"

echo "Done."
