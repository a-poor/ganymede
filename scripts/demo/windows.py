"""Capture visible windows of one process at their positions on the main display."""
import csv
import io
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import Quartz as Q
from PIL import Image

mode, pid, *args = sys.argv[1:]
pid = int(pid)
if mode == "backdrop":
    from AppKit import NSApp, NSApplication, NSColor, NSImage, NSImageView, NSScreen, NSWindow
    from Foundation import NSMakeRect

    app = NSApplication.sharedApplication()
    frame = NSScreen.screens()[0].frame()
    window = NSWindow.alloc().initWithContentRect_styleMask_backing_defer_(frame, 0, 2, False)
    window.setBackgroundColor_(NSColor.darkGrayColor())
    if args:
        view = NSImageView.alloc().initWithFrame_(NSMakeRect(0, 0, frame.size.width, frame.size.height))
        view.setImageScaling_(1)
        view.setImage_(NSImage.alloc().initWithContentsOfFile_(args[0]))
        window.setContentView_(view)
    window.orderFrontRegardless()
    app.run()



def windows():
    items = Q.CGWindowListCopyWindowInfo(Q.kCGWindowListOptionOnScreenOnly, 0)
    return [w for w in items if w.get("kCGWindowOwnerPID") == pid
            and w.get("kCGWindowLayer") == 0 and w.get("kCGWindowAlpha", 0) > 0]


def capture(window, path):
    result = subprocess.run(["/usr/sbin/screencapture", "-x", "-o", "-l",
                             str(window["kCGWindowNumber"]), str(path)],
                            capture_output=True, timeout=5)
    return result.returncode == 0 and path.is_file()


def locate(path, window, label):
    if label in {"Create", "Start server"}:
        with Image.open(path) as image:
            rgb = image.convert("RGB")
            # Primary submit button is the lowest saturated blue control.
            for y in range(rgb.height - 3, rgb.height // 2, -1):
                run = 0
                for x in range(rgb.width // 2, rgb.width):
                    r, g, blue = rgb.getpixel((x, y))
                    run = run + 1 if blue > 150 and blue - r > 100 and blue - g > 50 else 0
                    if run >= 24:
                        b = window["kCGWindowBounds"]
                        return (b["X"] + (x - 12) * b["Width"] / rgb.width,
                                b["Y"] + (y - 2) * b["Height"] / rgb.height)
    result = subprocess.run(["tesseract", str(path), "stdout", "--psm", "11", "tsv"],
                            capture_output=True, text=True, check=True, timeout=10)
    words = [r for r in csv.DictReader(io.StringIO(result.stdout), delimiter="\t")
             if r["level"] == "5"]
    count = len(label.split())
    with Image.open(path) as image:
        width, height = image.size
    for start, word in enumerate(words):
        group = words[start:start + count]
        if " ".join(r["text"] for r in group).casefold() == label.casefold():
            b = window["kCGWindowBounds"]
            return (b["X"] + (int(word["left"]) + int(word["width"])/2) * b["Width"]/width,
                    b["Y"] + (int(word["top"]) + int(word["height"])/2) * b["Height"]/height)


with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / "window.png"
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        for window in windows():
            if window["kCGWindowBounds"]["Width"] < 180:
                continue
            if capture(window, path) and (point := locate(path, window, args[0])):
                if mode == "click":
                    print(f"Clicking {args[0]} at {point}", flush=True)
                    Q.CGWarpMouseCursorPosition(point)
                    for kind in (Q.kCGEventMouseMoved, Q.kCGEventLeftMouseDown, Q.kCGEventLeftMouseUp):
                        event = Q.CGEventCreateMouseEvent(None, kind, point, Q.kCGMouseButtonLeft)
                        Q.CGEventPost(Q.kCGHIDEventTap, event)
                        time.sleep(.15)
                sys.exit(0)
        time.sleep(.25)
    if path.exists():
        import shutil
        shutil.copyfile(path, Path.cwd().parent / "last-window.png")
    sys.exit(f"Timed out waiting for {args[0]}")
