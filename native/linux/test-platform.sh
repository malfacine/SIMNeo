#!/bin/sh
# Requer Rust, dbus-run-session, gnome-keyring e secret-tool.
set -eu
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$project_root"
# Preservar o toolchain enquanto HOME aponta para o cofre descartável.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
test_home=$(mktemp -d /tmp/simneo-platform.XXXXXX)
export HOME="$test_home" XDG_DATA_HOME="$test_home/.local/share"
export XDG_CONFIG_HOME="$test_home/.config" XDG_RUNTIME_DIR="$test_home/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset GNOME_KEYRING_CONTROL
trap 'rm -rf -- "$test_home"' EXIT
dbus-run-session -- sh -c '
set -eu
printf "simneo-test-only" | gnome-keyring-daemon --unlock --components=secrets
cargo test --locked -p sim-core -p sim-p2p-t2u -p sim-storage-import -p sim-ui
'
