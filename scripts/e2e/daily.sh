#!/usr/bin/env bash
# Drive the real IDE through a day's worth of editing and check the files it
# leaves on disk. Every check here once failed in the real IDE while the unit
# tests passed.
#
#   scripts/e2e/daily.sh [path/to/phazeai-ui]     # default: target/debug/phazeai-ui
#
# Needs Linux with Xvfb, xdotool and git. The Rust checks also need a working
# rust-analyzer and rustfmt (they are reported as SKIP without them). Each
# run uses a fresh HOME, so your own config and session are never touched.
# Exits non-zero when any check fails. Set KEEP=1 to keep the work folder
# (projects, IDE logs) for a look after a failure.

set -u
BIN=${1:-target/debug/phazeai-ui}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
for tool in Xvfb xdotool git; do
    command -v "$tool" >/dev/null || { echo "missing $tool"; exit 2; }
done
[ -x "$BIN" ] || { echo "no IDE binary at $BIN (cargo build -p phazeai-ui)"; exit 2; }

WORK=$(mktemp -d)
DISPLAY_NUM=${DISPLAY_NUM:-77}
FAILED=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; echo "      $2"; FAILED=1; }
skip() { echo "SKIP  $1 ($2)"; }
oneline() { tr '\n' '|' < "$1" | sed 's/\r/\\r/g'; }

XVFB_PID=
IDE_PID=
cleanup() {
    [ -n "$IDE_PID" ] && kill "$IDE_PID" 2>/dev/null
    [ -n "$XVFB_PID" ] && kill "$XVFB_PID" 2>/dev/null
    [ "${KEEP:-0}" = 1 ] && echo "work folder kept: $WORK" || rm -rf "$WORK"
}
trap cleanup EXIT

Xvfb ":$DISPLAY_NUM" -screen 0 1400x900x24 >/dev/null 2>&1 & XVFB_PID=$!
sleep 2
export DISPLAY=":$DISPLAY_NUM" WAYLAND_DISPLAY=

# start_ide <project dir> [session.toml contents]
start_ide() {
    local proj=$1
    export HOME="$WORK/home-$(basename "$proj")" XDG_RUNTIME_DIR="$WORK/xdg-$(basename "$proj")"
    mkdir -p "$HOME/.config/phazeai" "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    [ -n "${2:-}" ] && printf '%s' "$2" > "$HOME/.config/phazeai/session.toml"
    (cd "$proj" && exec "$BIN") > "$proj.log" 2>&1 & IDE_PID=$!
    for _ in $(seq 1 60); do
        xdotool search --name PhazeAI >/dev/null 2>&1 && break
        sleep 1
    done
    sleep 8
    xdotool windowfocus --sync "$(xdotool search --name PhazeAI | head -1)" 2>/dev/null
}
stop_ide() {
    kill -INT "$IDE_PID" 2>/dev/null; sleep 2; kill "$IDE_PID" 2>/dev/null
    wait "$IDE_PID" 2>/dev/null; IDE_PID=
}
alive() { kill -0 "$IDE_PID" 2>/dev/null; }
# open_file <name typed into the file picker>; leaves the cursor in the editor.
open_file() {
    xdotool key ctrl+p; sleep 2; xdotool type --delay 80 "$1"; sleep 2; xdotool key Return; sleep 3
    xdotool mousemove 700 500 click 1; sleep 0.5
}
new_repo() {
    local dir=$1; mkdir -p "$dir"
    (cd "$dir" && git init -q && git add -A && git -c user.email=t@t -c user.name=t commit -qm init)
}

# ── Plain text: saving, outside changes, typing, find ────────────────────────
P="$WORK/text"; mkdir -p "$P"
printf 'call(\nblock {\nx\nend\n' > "$P/notes.txt"
printf 'alpha\nbeta\n' > "$P/other.txt"
new_repo "$P"
start_ide "$P"
open_file notes

xdotool key --delay 150 ctrl+Home End Down Home Down Down Up; sleep 3
[ "$(oneline "$P/notes.txt")" = "call(|block {|x|end|" ] \
    && pass "arrow keys change nothing" \
    || fail "arrow keys change nothing" "got $(oneline "$P/notes.txt")"

xdotool key ctrl+End; xdotool type --delay 120 "(edited)"
xdotool key ctrl+Home Down End Return; xdotool type --delay 120 "y"; xdotool key ctrl+s; sleep 2
[ "$(oneline "$P/notes.txt")" = "call(|block {|    y|x|end|(edited)" ] \
    && pass "typing: brackets, Enter after {, Ctrl+S" \
    || fail "typing: brackets, Enter after {, Ctrl+S" "got $(oneline "$P/notes.txt")"

printf 'call(\nblock {\n    y\nx\nend\n(edited)\nadded outside\n' > "$P/notes.txt"; sleep 3
xdotool key ctrl+Home End; xdotool type --delay 80 " Q"; xdotool key ctrl+s; sleep 2
[ "$(oneline "$P/notes.txt")" = "call( Q|block {|    y|x|end|(edited)|added outside|" ] \
    && pass "a change made outside the IDE survives the next save" \
    || fail "a change made outside the IDE survives the next save" "got $(oneline "$P/notes.txt")"

open_file other
xdotool key ctrl+f; sleep 1; xdotool type --delay 120 "beta"; sleep 1
xdotool key Escape; sleep 1; xdotool key ctrl+End; xdotool type --delay 80 "Z"; xdotool key ctrl+s; sleep 2
[ "$(oneline "$P/other.txt")" = "alpha|beta|Z" ] \
    && pass "Ctrl+F types into the find box; Escape gives the editor its keys back" \
    || fail "Ctrl+F types into the find box; Escape gives the editor its keys back" "got $(oneline "$P/other.txt")"

xdotool type --delay 80 "abc"; xdotool key ctrl+z ctrl+z ctrl+z; sleep 2
alive && pass "undo after typing" || fail "undo after typing" "the IDE exited (see $P.log)"
stop_ide

# ── Vim mode ────────────────────────────────────────────────────────────────
P="$WORK/vim"; mkdir -p "$P"; printf 'abc\ndef\n' > "$P/notes.txt"; new_repo "$P"
start_ide "$P" $'version = 1\nvim_mode = true\n'
open_file notes
xdotool key ctrl+Home; xdotool key Escape; sleep 0.5; xdotool key Escape; sleep 0.5
xdotool type --delay 150 "jx"; sleep 1; xdotool key ctrl+s; sleep 2
[ "$(oneline "$P/notes.txt")" = "abc|ef|" ] \
    && pass "Vim normal mode: j moves, x deletes, nothing is typed" \
    || fail "Vim normal mode: j moves, x deletes, nothing is typed" "got $(oneline "$P/notes.txt")"
stop_ide

# ── Rust: go to definition, rename, format on save ─────────────────────────
if ! rust-analyzer --version >/dev/null 2>&1 || ! rustfmt --version >/dev/null 2>&1; then
    skip "Rust checks" "rust-analyzer or rustfmt does not run"
else
    P="$WORK/my rust"; mkdir -p "$P/src"
    printf '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n' > "$P/Cargo.toml"
    printf 'mod util;\r\n\r\nfn main() {\r\n    let s = "h\xc3\xa9llo";\r\n    println!("{}", util::parse_config(s));\r\n}\r\n' > "$P/src/main.rs"
    printf 'pub fn parse_config(s: &str) -> usize {\n    s.len()\n}\n' > "$P/src/util.rs"
    printf 'pub fn messy( ){ }\n' > "$P/src/other.rs"
    new_repo "$P"
    start_ide "$P"
    open_file main.rs
    # Line 5, on `parse_config`; wait for rust-analyzer to index.
    xdotool key ctrl+Home Down Down Down Down; xdotool key --repeat 27 --delay 20 Right; sleep 45
    xdotool key F2; sleep 2; xdotool key ctrl+a; xdotool type --delay 60 "load_settings"; xdotool key Return; sleep 8
    if grep -q "util::load_settings(s)" "$P/src/main.rs" && grep -q "pub fn load_settings" "$P/src/util.rs"; then
        pass "F2 renames in both files"
    else
        fail "F2 renames in both files" "main.rs: $(oneline "$P/src/main.rs") util.rs: $(oneline "$P/src/util.rs")"
    fi
    [ "$(grep -c $'\r' "$P/src/main.rs")" = 6 ] \
        && pass "rename keeps CRLF and non-ASCII text" \
        || fail "rename keeps CRLF and non-ASCII text" "got $(oneline "$P/src/main.rs")"

    xdotool key ctrl+Home Down Down Down Down; xdotool key --repeat 27 --delay 20 Right; sleep 3
    xdotool key F12; sleep 5; xdotool type --delay 80 "Z"; xdotool key ctrl+s; sleep 3
    # The cursor lands on the definition's first line (at `pub fn`, not
    # yet on the name).
    head -1 "$P/src/util.rs" | grep -q "^Z\?pub fn Z\?load_settings\|^Zpub fn load_settings" \
        && pass "F12 opens the definition in another file" \
        || fail "F12 opens the definition in another file" "util.rs: $(oneline "$P/src/util.rs")"

    open_file main.rs
    [ -n "${SHOTS:-}" ] && import -window root "$SHOTS/fmt-1-open.png"
    xdotool key ctrl+End; xdotool type --delay 60 "fn extra(){let a=1;}"; sleep 1
    [ -n "${SHOTS:-}" ] && import -window root "$SHOTS/fmt-2-typed.png"
    xdotool key ctrl+s; sleep 6
    if grep -q "fn extra() {" "$P/src/main.rs" && [ "$(grep -c $'\r' "$P/src/main.rs")" -ge 6 ] \
        && [ "$(cat "$P/src/other.rs")" = "pub fn messy( ){ }" ]; then
        pass "format on save: formats the file, keeps CRLF, leaves other files alone"
    else
        fail "format on save: formats the file, keeps CRLF, leaves other files alone" \
            "main.rs: $(oneline "$P/src/main.rs") other.rs: $(oneline "$P/src/other.rs")"
    fi
    alive && pass "IDE still running after the Rust checks" || fail "IDE still running after the Rust checks" "see $P.log"
    stop_ide
fi

[ "$FAILED" = 0 ] && echo "all checks passed" || echo "some checks FAILED"
exit "$FAILED"
