#!/usr/bin/env python3
"""A plain window for trying the layout: its name, big, on its own colour.

    tools/testwin.py NAME [MIN_WIDTH]
"""
import sys
import zlib

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

NAME = sys.argv[1] if len(sys.argv) > 1 else "Window"
MIN_WIDTH = int(sys.argv[2]) if len(sys.argv) > 2 else 0
HUE = zlib.crc32(NAME.encode()) % 360

CSS = f"""
window {{ background: hsl({HUE}, 55%, 42%); }}
label.name {{ color: white; font-size: 48px; font-weight: 700; }}
label.size {{ color: rgba(255,255,255,0.75); font-size: 18px; }}
"""


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title=NAME)
    provider = Gtk.CssProvider()
    provider.load_from_string(CSS)
    Gtk.StyleContext.add_provider_for_display(win.get_display(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, valign=Gtk.Align.CENTER, spacing=8)
    name = Gtk.Label(label=NAME, css_classes=["name"])
    size = Gtk.Label(css_classes=["size"])
    box.append(name)
    box.append(size)
    win.set_child(box)
    if MIN_WIDTH:
        box.set_size_request(MIN_WIDTH, -1)

    def tick(widget, _clock):
        size.set_label(f"{widget.get_width()} × {widget.get_height()}")
        return True

    win.add_tick_callback(tick)
    win.present()


app = Gtk.Application(application_id=None)
app.connect("activate", activate)
app.run([])
