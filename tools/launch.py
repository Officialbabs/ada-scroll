#!/usr/bin/env python3
"""Scrollvil's app picker: type to find an installed app, Enter opens it.

Apps opened here open inside Scrollvil (they inherit its WAYLAND_DISPLAY).
"""
import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gio, Gtk  # noqa: E402


def apps():
    found = [a for a in Gio.AppInfo.get_all() if a.should_show()]
    return sorted(found, key=lambda a: a.get_display_name().casefold())


def matches(app, words):
    text = " ".join(
        filter(None, [app.get_display_name(), app.get_description() or "", app.get_executable() or ""])
    ).casefold()
    return all(w in text for w in words)


def activate(gtk_app):
    win = Gtk.ApplicationWindow(application=gtk_app, title="Open an app", default_width=480, default_height=440)
    # Fixed size: Scrollvil floats it in the middle instead of tiling it.
    win.set_resizable(False)

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, margin_top=12, margin_bottom=12,
                  margin_start=12, margin_end=12)
    search = Gtk.SearchEntry(placeholder_text="Search apps")
    listbox = Gtk.ListBox(selection_mode=Gtk.SelectionMode.BROWSE, css_classes=["navigation-sidebar"])
    scroller = Gtk.ScrolledWindow(vexpand=True, child=listbox)
    box.append(search)
    box.append(scroller)
    win.set_child(box)

    rows = []
    for app in apps():
        row = Gtk.ListBoxRow()
        line = Gtk.Box(spacing=10, margin_top=4, margin_bottom=4)
        icon = Gtk.Image(pixel_size=24)
        if app.get_icon():
            icon.set_from_gicon(app.get_icon())
        line.append(icon)
        line.append(Gtk.Label(label=app.get_display_name(), xalign=0))
        row.set_child(line)
        row.app = app
        listbox.append(row)
        rows.append(row)

    def first_shown():
        return next((r for r in rows if r.get_visible()), None)

    def refilter(*_):
        words = search.get_text().casefold().split()
        for row in rows:
            row.set_visible(matches(row.app, words))
        listbox.select_row(first_shown())

    def open_row(row):
        if row is None:
            return
        try:
            row.app.launch([], None)
        except Exception as err:  # the app's own launch failed; say so
            search.set_text("")
            search.set_placeholder_text(f"Couldn't open it: {err}")
            return
        win.close()

    def keys(_ctl, keyval, _code, _state):
        if keyval == Gdk.KEY_Escape:
            win.close()
            return True
        return False

    search.connect("search-changed", refilter)
    search.connect("activate", lambda *_: open_row(listbox.get_selected_row() or first_shown()))
    listbox.connect("row-activated", lambda _l, row: open_row(row))
    controller = Gtk.EventControllerKey()
    controller.connect("key-pressed", keys)
    win.add_controller(controller)
    refilter()
    win.present()
    search.grab_focus()


app = Gtk.Application(application_id=None, flags=Gio.ApplicationFlags.NON_UNIQUE)
app.connect("activate", activate)
app.run([])
