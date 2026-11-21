import sys
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gdk

log_path = sys.argv[1]
app = Gtk.Application(application_id="local.rscu.Smoke", flags=32)

def setup(app):
    main = Gtk.ApplicationWindow(application=app, title="rscu-main")
    main.set_default_size(620, 400)
    button = Gtk.Button(label="Open modal")
    main.set_child(button)
    other = Gtk.ApplicationWindow(application=app, title="rscu-other")
    other.set_default_size(350, 240)
    menu = Gtk.MenuButton(label="Open menu")
    popover = Gtk.Popover()
    item = Gtk.Button(label="Select item")
    item.connect("clicked", lambda _: popover.popdown())
    popover.set_child(item)
    menu.set_popover(popover)
    other.set_child(menu)
    other_keys = Gtk.EventControllerKey()

    def resize_other(_, key, *_args):
        if key == Gdk.KEY_m:
            other.maximize()
            return True
        if key == Gdk.KEY_u:
            other.unmaximize()
            return True
        return False

    other_keys.connect("key-pressed", resize_other)
    other.add_controller(other_keys)
    other.present()

    def show_dialog(*_):
        dialog = Gtk.Window(application=app, title="rscu-modal", transient_for=main, modal=True)
        dialog.set_default_size(260, 180)
        dialog.set_child(Gtk.Button(label="Close"))
        dialog.get_child().connect("clicked", lambda _: dialog.close())
        keyboard = Gtk.EventControllerKey()
        def on_key(_, key, *_args):
            if key == Gdk.KEY_a:
                with open(log_path, "a") as log:
                    log.write("modal received a\n")
                dialog.close()
                button.set_label("Modal closed with a")
                return True
            return False
        keyboard.connect("key-pressed", on_key)
        dialog.add_controller(keyboard)
        dialog.present()

    button.connect("clicked", show_dialog)
    main.present()
app.connect("activate", setup)
app.run([])
