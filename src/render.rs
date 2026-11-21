use crate::{
    session::{Result, SIZE, Session},
    window::SceneWindow,
};
use smithay::{
    backend::{
        allocator::{Fourcc, dmabuf::Dmabuf},
        egl::{EGLContext, EGLDevice, EGLDisplay, native::EGLSurfacelessDisplay},
        renderer::{
            ExportMem, ImportAll, ImportDma, ImportEgl, Offscreen, Texture,
            damage::OutputDamageTracker,
            element::{AsRenderElements, surface::WaylandSurfaceRenderElement},
            gles::{GlesRenderbuffer, GlesRenderer, ffi},
            pixman::PixmanRenderer,
        },
    },
    desktop::{Space, space::render_output},
    output::Output,
    reexports::wayland_server::DisplayHandle,
    utils::{Logical, Point, Rectangle},
    wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState},
};
use std::{ffi::CStr, os::unix::fs::MetadataExt, path::PathBuf};

#[derive(Clone, Copy)]
pub enum Backend {
    Pixman,
    Gles,
}

pub enum Renderer {
    Pixman(Box<PixmanRenderer>),
    Gles(Box<GlesRenderer>),
}

impl Renderer {
    pub fn new(
        backend: Backend,
        display: &DisplayHandle,
        dmabuf: &mut DmabufState,
    ) -> Result<Self> {
        match backend {
            Backend::Pixman => {
                eprintln!("Renderer: Pixman (CPU)");
                Ok(Self::Pixman(Box::new(PixmanRenderer::new()?)))
            }
            Backend::Gles => {
                let render_node = std::env::var_os("RSCU_RENDER_NODE").map(PathBuf::from);
                let device = EGLDevice::enumerate()?.find(|device| match &render_node {
                    Some(requested) => device
                        .render_device_path()
                        .is_ok_and(|path| path == *requested),
                    None => device.render_device_path().is_ok() || device.drm_device_path().is_ok(),
                });
                let node = device.as_ref().and_then(|device| {
                    device
                        .render_device_path()
                        .or_else(|_| device.drm_device_path())
                        .ok()
                });
                // The device/platform stays owned by the EGL display. The context and
                // renderer are used only on the compositor thread.
                let egl = unsafe {
                    match device {
                        Some(device) => EGLDisplay::new(device)?,
                        None if render_node.is_none() => EGLDisplay::new(EGLSurfacelessDisplay)?,
                        None => return Err("requested EGL render node was not found".into()),
                    }
                };
                let context = EGLContext::new(&egl)?;
                let mut renderer = unsafe { GlesRenderer::new(context)? };
                let renderer_name = renderer.with_context(|gl| {
                    // GL_RENDERER lives as long as the current GL context.
                    let name = unsafe { gl.GetString(ffi::RENDERER) };
                    if name.is_null() {
                        String::from("unknown")
                    } else {
                        unsafe { CStr::from_ptr(name.cast()) }
                            .to_string_lossy()
                            .into_owned()
                    }
                })?;
                eprintln!(
                    "Renderer: {renderer_name} ({})",
                    node.as_deref()
                        .map(|node| node.display().to_string())
                        .unwrap_or_else(|| "surfaceless EGL".into())
                );

                if let Err(error) = renderer.bind_wl_display(display) {
                    eprintln!("EGL Wayland buffer interface: {error}");
                }
                let formats = renderer.dmabuf_formats();
                if formats.iter().next().is_some() {
                    let feedback =
                        node.and_then(|node| std::fs::metadata(node).ok())
                            .and_then(|metadata| {
                                DmabufFeedbackBuilder::new(metadata.rdev(), formats.clone())
                                    .build()
                                    .ok()
                            });
                    if let Some(feedback) = feedback {
                        dmabuf.create_global_with_default_feedback::<Session>(display, &feedback);
                    } else {
                        dmabuf.create_global::<Session>(display, formats);
                    }
                }
                Ok(Self::Gles(Box::new(renderer)))
            }
        }
    }

    pub fn import_dmabuf(&mut self, buffer: &Dmabuf) -> bool {
        match self {
            Self::Gles(renderer) => renderer.import_dmabuf(buffer, None).is_ok(),
            Self::Pixman(_) => false,
        }
    }

    pub fn capture(
        &mut self,
        output: &Output,
        space: &Space<SceneWindow>,
        area: Rectangle<i32, Logical>,
    ) -> Result<Vec<u8>> {
        match self {
            Self::Pixman(renderer) => capture(
                renderer.as_mut(),
                output,
                Scene::Active(space),
                area,
                Fourcc::Xrgb8888,
            ),
            Self::Gles(renderer) => capture::<_, GlesRenderbuffer>(
                renderer.as_mut(),
                output,
                Scene::Active(space),
                area,
                Fourcc::Abgr8888,
            ),
        }
    }
    pub fn capture_group(
        &mut self,
        output: &Output,
        group: &[(SceneWindow, Point<i32, Logical>)],
        area: Rectangle<i32, Logical>,
    ) -> Result<Vec<u8>> {
        match self {
            Self::Pixman(renderer) => capture(
                renderer.as_mut(),
                output,
                Scene::Group(group),
                area,
                Fourcc::Xrgb8888,
            ),
            Self::Gles(renderer) => capture::<_, GlesRenderbuffer>(
                renderer.as_mut(),
                output,
                Scene::Group(group),
                area,
                Fourcc::Abgr8888,
            ),
        }
    }
}

enum Scene<'a> {
    Active(&'a Space<SceneWindow>),
    Group(&'a [(SceneWindow, Point<i32, Logical>)]),
}

fn capture<R, T>(
    renderer: &mut R,
    output: &Output,
    scene: Scene<'_>,
    area: Rectangle<i32, Logical>,
    format: Fourcc,
) -> Result<Vec<u8>>
where
    R: ImportAll + Offscreen<T> + ExportMem,
    R::Error: 'static,
    R::TextureId: Clone + Texture + 'static,
{
    let mut buffer = renderer.create_buffer(format, SIZE.into())?;
    let mut target = renderer.bind(&mut buffer)?;
    let mut tracker = OutputDamageTracker::from_output(output);
    let clear = [0.0, 0.0, 0.0, 1.0];
    let sync = match scene {
        Scene::Active(space) => {
            render_output::<_, WaylandSurfaceRenderElement<R>, _, _>(
                output,
                renderer,
                &mut target,
                1.0,
                0,
                [space],
                &[],
                &mut tracker,
                clear,
            )?
            .sync
        }
        Scene::Group(group) => {
            let elements: Vec<WaylandSurfaceRenderElement<R>> = group
                .iter()
                .rev()
                .flat_map(|(window, location)| {
                    let origin =
                        *location - smithay::desktop::space::SpaceElement::geometry(window).loc;
                    window.render_elements(
                        renderer,
                        origin.to_physical_precise_round(1.0),
                        1.0.into(),
                        1.0,
                    )
                })
                .collect();
            tracker
                .render_output(renderer, &mut target, 0, &elements, clear)?
                .sync
        }
    };
    sync.wait()?;
    // With a Normal Smithay transform on an offscreen target, logical row zero
    // is the first row read by both Pixman and GLES.
    let mapping = renderer.copy_framebuffer(
        &target,
        Rectangle::new(
            (area.loc.x, area.loc.y).into(),
            (area.size.w, area.size.h).into(),
        ),
        format,
    )?;
    let pixels = renderer.map_texture(&mapping)?;
    let mut rgb = Vec::with_capacity(area.size.w as usize * area.size.h as usize * 3);
    for rgba in pixels.chunks_exact(4) {
        let word = u32::from_ne_bytes([rgba[0], rgba[1], rgba[2], rgba[3]]);
        let (red, blue) = if format == Fourcc::Abgr8888 {
            (word, word >> 16)
        } else {
            (word >> 16, word)
        };
        rgb.extend_from_slice(&[red as u8, (word >> 8) as u8, blue as u8]);
    }
    Ok(rgb)
}
