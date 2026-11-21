use smithay::{
    backend::renderer::{
        ImportAll,
        element::{
            AsRenderElements, Kind,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
        },
    },
    desktop::{
        Window, WindowSurfaceType,
        space::SpaceElement,
        utils::{bbox_from_surface_tree, send_frames_surface_tree, under_from_surface_tree},
    },
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale},
    wayland::seat::WaylandFocus,
};
use std::{ops::Deref, time::Duration};

/// An X11 window on pre-23.1 XWayland identifies its surface by protocol ID.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneWindow {
    window: Window,
    pub legacy_surface: Option<WlSurface>,
}

impl From<Window> for SceneWindow {
    fn from(window: Window) -> Self {
        Self {
            window,
            legacy_surface: None,
        }
    }
}

impl Deref for SceneWindow {
    type Target = Window;

    fn deref(&self) -> &Window {
        &self.window
    }
}

impl SceneWindow {
    pub fn surface(&self) -> Option<WlSurface> {
        self.legacy_surface
            .clone()
            .or_else(|| self.window.wl_surface().map(|surface| surface.into_owned()))
    }

    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        self.legacy_surface.as_ref().map_or_else(
            || self.window.bbox_with_popups(),
            |surface| bbox_from_surface_tree(surface, (0, 0)),
        )
    }

    pub fn surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        if let Some(surface) = &self.legacy_surface {
            under_from_surface_tree(surface, position, (0, 0), WindowSurfaceType::ALL)
        } else {
            self.window.surface_under(position, WindowSurfaceType::ALL)
        }
    }

    pub fn send_frame(&self, output: &Output, time: Duration) {
        let scanout =
            |_: &WlSurface, _: &smithay::wayland::compositor::SurfaceData| Some(output.clone());
        if let Some(surface) = &self.legacy_surface {
            send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), scanout);
        } else {
            self.window
                .send_frame(output, time, Some(Duration::ZERO), scanout);
        }
    }
}

impl IsAlive for SceneWindow {
    fn alive(&self) -> bool {
        self.window.alive()
    }
}

impl SpaceElement for SceneWindow {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        if self.legacy_surface.is_some() {
            self.bbox_with_popups()
        } else {
            self.window.geometry()
        }
    }

    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.bbox_with_popups()
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.surface_under(*point).is_some()
    }

    fn z_index(&self) -> u8 {
        self.window.z_index()
    }

    fn set_activate(&self, activated: bool) {
        if self.window.is_wayland() {
            self.window.set_activated(activated);
        }
    }

    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        if let Some(surface) = &self.legacy_surface {
            output.enter(surface);
        } else {
            self.window.output_enter(output, overlap);
        }
    }

    fn output_leave(&self, output: &Output) {
        if let Some(surface) = &self.legacy_surface {
            output.leave(surface);
        } else {
            self.window.output_leave(output);
        }
    }

    fn refresh(&self) {
        self.window.refresh();
    }
}

impl<R: ImportAll> AsRenderElements<R> for SceneWindow
where
    R::TextureId: Clone + 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        if let Some(surface) = &self.legacy_surface {
            render_elements_from_surface_tree(
                renderer,
                surface,
                location,
                scale,
                alpha,
                Kind::Unspecified,
            )
        } else {
            self.window
                .render_elements(renderer, location, scale, alpha)
        }
    }
}
