"""Record the mouse events that reach a real GTK client."""

import sys
from pathlib import Path

import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk

log = Path(sys.argv[1])
app = Gtk.Application(application_id="local.rscu.MouseSmoke", flags=32)


def record(kind, *values):
    with log.open("a") as events:
        print(kind, *values, file=events)


def setup(application):
    window = Gtk.ApplicationWindow(application=application, title="rscu-mouse", decorated=False)
    window.set_default_size(320, 240)
    area = Gtk.DrawingArea()
    window.set_child(area)

    motion = Gtk.EventControllerMotion()
    motion.connect("enter", lambda _, x, y: record("motion", int(x), int(y)))
    motion.connect("motion", lambda _, x, y: record("motion", int(x), int(y)))
    area.add_controller(motion)

    click = Gtk.GestureClick(button=0)
    click.connect("pressed", lambda gesture, count, *_: record("press", gesture.get_current_button(), count))
    area.add_controller(click)

    scroll = Gtk.EventControllerScroll(flags=Gtk.EventControllerScrollFlags.BOTH_AXES)
    def scrolled(_, dx, dy):
        record("scroll", dx, dy)
        return True
    scroll.connect("scroll", scrolled)
    area.add_controller(scroll)

    drag = Gtk.GestureDrag(button=1)
    drag.connect("drag-end", lambda _, dx, dy: record("drag", int(dx), int(dy)))
    area.add_controller(drag)

    window.present()


app.connect("activate", setup)
app.run([])
