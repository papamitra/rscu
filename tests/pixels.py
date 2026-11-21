import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk

app = Gtk.Application(application_id="local.rscu.Pixels", flags=32)
def setup(app):
    window = Gtk.ApplicationWindow(application=app, title="rscu-pixels", decorated=False)
    window.set_default_size(300, 200)
    area = Gtk.DrawingArea()
    def draw(_, cr, width, height):
        cr.set_source_rgb(1, 0, 0)
        cr.rectangle(0, 0, width, height / 2)
        cr.fill()
        cr.set_source_rgb(0, 0, 1)
        cr.rectangle(0, height / 2, width, height / 2)
        cr.fill()
    area.set_draw_func(draw)
    window.set_child(area)
    window.present()
app.connect("activate", setup)
app.run([])
