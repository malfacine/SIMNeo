#!/bin/sh
# Perfil de DVR por stdin, nunca em argumentos ou arquivo descriptografado.
set -eu
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$project_root"
export SIMNEO_LINUX_BIN_DIR="${1:?Informe a pasta dos binários Linux}"
export SIMNEO_SMOKE_MODE="${2:-all}"
export SIMNEO_LINUX_APPIMAGE="${3:-}"
if [ -n "$SIMNEO_LINUX_APPIMAGE" ]; then
    [ -x "$SIMNEO_LINUX_APPIMAGE" ] || { echo 'AppImage inexistente ou sem permissão de execução' >&2; exit 2; }
fi
for required in dbus-run-session gnome-keyring-daemon secret-tool xvfb-run ffprobe ffmpeg; do
    command -v "$required" >/dev/null 2>&1 || {
        printf 'Dependência de teste ausente: %s\n' "$required" >&2
        exit 2
    }
done
test_home=$(mktemp -d /tmp/simneo-ui.XXXXXX)
export HOME="$test_home" XDG_DATA_HOME="$test_home/.local/share"
export XDG_CONFIG_HOME="$test_home/.config" XDG_RUNTIME_DIR="$test_home/run"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset GNOME_KEYRING_CONTROL WAYLAND_DISPLAY
export SLINT_BACKEND=winit-software WINIT_UNIX_BACKEND=x11
export SIMNEO_P2P_LIBRARY="$project_root/native/linux/t2u/build/libsim_t2u_linux.so"
if [ -n "$SIMNEO_LINUX_APPIMAGE" ]; then unset SIMNEO_P2P_LIBRARY; fi
trap 'rm -rf -- "$test_home"' EXIT
dbus-run-session -- sh -c '
set -eu
printf "simneo-ui-test-only" | gnome-keyring-daemon --unlock --components=secrets
"$SIMNEO_LINUX_BIN_DIR/examples/linux_seed" "$HOME/test.db"
case "$SIMNEO_SMOKE_MODE" in
    all) set -- --connection-smoke-test --video-smoke-test --mosaic-smoke-test --playback-smoke-test ;;
    mosaic) set -- --mosaic-smoke-test ;;
    video) set -- --video-smoke-test ;;
    playback) set -- --playback-smoke-test ;;
    *) echo "Modo de teste inválido" >&2; exit 2 ;;
esac
status=0
for mode do
    printf "Teste Linux: %s\n" "$mode"
    app="${SIMNEO_LINUX_APPIMAGE:-$SIMNEO_LINUX_BIN_DIR/sim-ui}"
    xvfb-run -a -s "-screen 0 1920x1080x24" "$app" --db "$HOME/test.db" "$mode" || { status=$?; break; }
done
for video in "$HOME"/playback-ui-*.mp4; do
    [ -f "$video" ] || continue
    ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height,duration -of json "$video"
    ffmpeg -v error -xerror -i "$video" -f null -
done
exit "$status"
'
