#!/bin/bash
# macOS 14+. Run from a logged-in desktop with the main display available.
set -euo pipefail
src="$(cd "$(dirname "$0")" && pwd)"
out="$(mktemp -d "${1:-/private/tmp}/ganymede-demo.XXXXXX")"
echo "Demo files: $out"
app="${GANYMEDE_DEMO_APP:-/Applications/Ganymede.app}/Contents/MacOS/Ganymede"
test -x "$app"
encoder="$(command -v ffmpeg)"
python_bin="${GANYMEDE_DEMO_PYTHON:-$out/runtime/bin/python}"
if [[ -z "${GANYMEDE_DEMO_PYTHON:-}" ]]; then
    uv venv "$out/runtime"
    uv pip install --python "$python_bin" jupyter-server ipykernel requests
fi
"$python_bin" -c 'import jupyter_server, ipykernel, requests'
uv venv "$out/capture-runtime"
driver="$out/capture-runtime/bin/python"
uv pip install --python "$driver" Pillow pyobjc-framework-Quartz
helper() { "$driver" "$src/windows.py" "$@"; }
mkdir -p "$out/notebooks" "$out/bin"
ln -s "$python_bin" "$out/bin/python3"
backdrop_args=()
if [[ -n "${GANYMEDE_DEMO_BACKGROUND:-}" ]]; then
    backdrop_args+=("$GANYMEDE_DEMO_BACKGROUND")
else
    for image in "$HOME"/Downloads/"Screenshot 2026-11-17 at 3.57.52"*PM.png; do
        [[ ! -f "$image" ]] || backdrop_args+=("$image")
    done
fi
"$driver" "$src/windows.py" backdrop 0 "${backdrop_args[@]}" &
backdrop=$!
sleep 1
cd "$out/notebooks"
pid=""
rec=""
finish() {
    if [[ -n "$rec" ]]; then
        kill -INT "$rec" 2>/dev/null || true
        wait "$rec" || [[ $? == 255 ]]
        rec=""
    fi
    kill "$backdrop" "$pid" 2>/dev/null || true
}
trap finish EXIT
device=$("$encoder" -hide_banner -f avfoundation -list_devices true -i '' 2>&1 | sed -n 's/.*\[\([0-9]*\)\] Capture screen 0$/\1/p' || true)
[[ -n "$device" ]] || { echo "Main display unavailable" >&2; exit 1; }
"$encoder" -nostdin -loglevel error -f avfoundation -framerate 60 -pixel_format nv12 -capture_cursor 1 \
    -i "$device:none" -vf 'crop=min(iw\,1920):min(ih\,1080),scale=1920:1080,setsar=1' \
    -r 60 -c:v h264_videotoolbox -b:v 12M -pix_fmt yuv420p -movflags +faststart "$out/demo.mp4" &
rec=$!
sleep 1
kill -0 "$rec"
PATH="$(dirname "$python_bin"):$out/bin:$PATH" GANYMEDE_CONNECTIONS_FILE="$out/connections.json" GANYMEDE_PREFERENCES_FILE="$out/preferences.json" "$app" >"$out/application.log" 2>&1 &
pid=$!
sleep 2
osascript -e "tell application \"System Events\" to set frontmost of first application process whose unix id is $pid to true"
locate() {
    echo "Waiting to $1: $2"
    helper "$1" "$pid" "$2"
    sleep .5
}
key() {
    osascript -e "tell application \"System Events\" to tell (first application process whose unix id is $pid) to $1"
    sleep .25
}
locate click Create
locate click 'Start server'
locate click 'Python 3'
locate see Code
key 'key code 53'
key 'keystroke "m"'
key 'key code 36'
# Run-and-insert opens the next cell in edit mode.
key 'keystroke "# Welcome to Ganymede"'
key 'key code 36'
key 'key code 36'
key 'keystroke "A fast native Jupyter desktop client, written in Rust."'
key 'key code 36 using option down'
key 'keystroke "sum(range(1, 11))"'
key 'key code 36 using option down'
key 'keystroke "import requests"'
key 'key code 36'
key 'keystroke "from time import sleep"'
key 'key code 36'
key 'key code 36'
key 'keystroke "url = \"https://jsonplaceholder.typicode.com/posts?userId=1\""'
key 'key code 36'
key 'keystroke "url"'
key 'key code 36 using option down'
key 'keystroke "response = requests.get(url, timeout=10)"'
key 'key code 36'
key 'keystroke "response.raise_for_status()"'
key 'key code 36'
key 'keystroke "sleep(2)  # Keep the busy indicator visible for the demo"'
key 'key code 36'
key 'keystroke "posts = response.json()"'
key 'key code 36'
key 'keystroke "posts[0][\"title\"]"'
key 'key code 36 using command down'
"$python_bin" - <<'PYTHON'
import json
import time
from pathlib import Path

for attempt in range(80):
    for notebook in Path.cwd().glob("*.ipynb"):
        try:
            cells = json.loads(notebook.read_text())["cells"]
        except (OSError, ValueError):
            continue
        expected = ["55", "'https://jsonplaceholder.typicode.com/posts?userId=1'", "'sunt aut facere repellat provident occaecati excepturi optio reprehenderit'"]
        results = ["".join(o.get("data", {}).get("text/plain", []))
                   for c in cells for o in c.get("outputs", [])]
        if (len(cells) == 4 and "".join(cells[1]["source"]) == "sum(range(1, 11))"
                and results == expected):
            raise SystemExit(0)
    time.sleep(.5)
raise SystemExit("Timed out waiting for the notebook to save all three expected results")
PYTHON
osascript -e "tell application \"System Events\" to tell (first application process whose unix id is $pid) to click menu item \"Rename…\" of menu 1 of menu bar item \"File\" of menu bar 1"
locate see 'Rename file'
key 'keystroke "a" using command down'
key 'keystroke "demo.ipynb"'
sleep .6
key 'key code 36'
for ((i=0; i<40; i++)); do
    [[ -f demo.ipynb && ! -e Untitled.ipynb ]] && break
    sleep .25
done
[[ -f demo.ipynb && ! -e Untitled.ipynb ]] || { echo "Notebook rename failed" >&2; exit 1; }
sleep 3
finish
trap - EXIT
echo "Recording: $out/demo.mp4"
