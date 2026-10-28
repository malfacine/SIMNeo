#!/bin/sh
set -eu
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$project_root"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/simneo-target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
make -C native/linux/t2u all
cargo build --locked --release -p sim-ui --bin sim-ui
python3 native/linux/appimage/package.py --binary "$CARGO_TARGET_DIR/release/sim-ui"
