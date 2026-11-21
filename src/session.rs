use crate::{
    control::{self, Request},
    focus::KeyboardFocus,
    render::{Backend, Renderer},
    window::SceneWindow,
};
use serde_json::{Value, json};
use smithay::{
    backend::{
        input::{ButtonState, KeyState},
        renderer::buffer_dimensions,
    },
    desktop::{PopupManager, Space, Window, space::SpaceElement},
    input::{
        Seat, SeatState,
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::{
            Client, Display, DisplayHandle, ListeningSocket, protocol::wl_surface::WlSurface,
        },
    },
    utils::{IsAlive, Logical, Rectangle, SERIAL_COUNTER, Transform},
    wayland::{
        compositor::{CompositorState, with_states},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::{XdgShellState, XdgToplevelSurfaceData},
        shm::ShmState,
    },
    xwayland::{X11Wm, XWayland},
};
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const SIZE: (i32, i32) = (1280, 800);

pub struct Entry {
    pub id: u64,
    pub window: SceneWindow,
    pub restore: Option<Rectangle<i32, Logical>>,
    pub popup_owner: Option<u64>,
}

struct Capture {
    id: u64,
    window: u64,
    area: Rectangle<i32, Logical>,
    window_area: Rectangle<i32, Logical>,
    epoch: u64,
    hash: u64,
}

pub struct Session {
    pub dh: DisplayHandle,
    pub compositor: CompositorState,
    pub xdg: XdgShellState,
    pub shm: ShmState,
    pub seats: SeatState<Self>,
    pub data_device: DataDeviceState,
    pub seat: Seat<Self>,
    pub popups: PopupManager,
    pub windows: Vec<Entry>,
    pub space: Space<SceneWindow>,
    pub xwm: Option<X11Wm>,
    xwayland_client: Option<Client>,
    x11_properties: Option<X11Properties>,
    pub xwayland_shell: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub output: Output,
    pub dmabuf: smithay::wayland::dmabuf::DmabufState,
    selected: Option<u64>,
    focused: Option<u64>,
    epoch: u64,
    next_window: u64,
    next_image: u64,
    captures: VecDeque<Capture>,
    pub renderer: Renderer,
    capture_dir: PathBuf,
    start: Instant,
    stopped: bool,
    click_step: Option<(Instant, ButtonState, u64)>,
    children: Vec<Child>,
}

impl Session {
    fn new(dh: DisplayHandle, capture_dir: PathBuf, backend: Backend) -> Result<Self> {
        let compositor = CompositorState::new::<Self>(&dh);
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::WmCapabilities;
        let xdg = XdgShellState::new_with_capabilities::<Self>(
            &dh,
            [WmCapabilities::Maximize, WmCapabilities::Fullscreen],
        );
        smithay::wayland::shell::xdg::dialog::XdgDialogState::new::<Self>(&dh);
        let shm = ShmState::new::<Self>(&dh, vec![]);
        let xwayland_shell = smithay::wayland::xwayland_shell::XWaylandShellState::new::<Self>(&dh);
        OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device = DataDeviceState::new::<Self>(&dh);
        let mut seats = SeatState::new();
        let mut seat = seats.new_wl_seat(&dh, "rscu");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();

        let output = Output::new(
            "rscu-1".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "rscu".into(),
                model: "virtual".into(),
            },
        );
        output.create_global::<Self>(&dh);
        let mode = Mode {
            size: SIZE.into(),
            refresh: 60_000,
        };
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        output.set_preferred(mode);
        let mut space = Space::default();
        space.map_output(&output, (0, 0));

        let mut dmabuf = smithay::wayland::dmabuf::DmabufState::new();
        let renderer = Renderer::new(backend, &dh, &mut dmabuf)?;
        Ok(Self {
            dh,
            compositor,
            xdg,
            shm,
            seats,
            data_device,
            seat,
            popups: PopupManager::default(),
            windows: Vec::new(),
            space,
            xwm: None,
            xwayland_client: None,
            x11_properties: None,
            xwayland_shell,
            output,
            selected: None,
            focused: None,
            epoch: 0,
            next_window: 1,
            next_image: 1,
            captures: VecDeque::new(),
            renderer,
            dmabuf,
            capture_dir,
            start: Instant::now(),
            stopped: false,
            click_step: None,
            children: Vec::new(),
        })
    }

    pub fn insert(&mut self, window: Window) {
        let popup_owner = window
            .x11_surface()
            .filter(|x11| x11.is_override_redirect())
            .and_then(|popup| self.x11_popup_owner(popup));
        self.windows.push(Entry {
            id: self.next_window,
            window: window.into(),
            restore: None,
            popup_owner,
        });
        self.next_window += 1;
    }

    fn x11_popup_owner(&self, popup: &smithay::xwayland::X11Surface) -> Option<u64> {
        let properties = self.x11_properties.as_ref()?;
        if let Some(transient_for) = popup.is_transient_for()
            && let Some(owner) = self.windows.iter().find(|e| {
                e.window
                    .x11_surface()
                    .is_some_and(|x| x.window_id() == transient_for)
            })
        {
            return Some(owner.id);
        }
        let belongs_to = |entry: &&Entry| {
            entry.window.x11_surface().is_some_and(|x| {
                !x.is_override_redirect()
                    && properties.same_client(popup.window_id(), x.window_id())
            })
        };
        self.focused
            .and_then(|id| self.entry(id))
            .filter(belongs_to)
            .map(|entry| entry.id)
            .or_else(|| {
                let mut owners = self.windows.iter().filter(belongs_to);
                let owner = owners.next()?;
                owners.next().is_none().then_some(owner.id)
            })
    }

    fn entry(&self, id: u64) -> Option<&Entry> {
        self.windows.iter().find(|entry| entry.id == id)
    }

    pub fn focus_matches(&self, surface: &WlSurface) -> bool {
        self.focused
            .and_then(|id| self.entry(id))
            .and_then(|e| e.window.surface())
            .as_ref()
            == Some(surface)
    }

    // Look up legacy IDs only inside the client connection created by XWayland.
    #[allow(deprecated)]
    fn associate_legacy_surfaces(&mut self) {
        let Some(client) = &self.xwayland_client else {
            return;
        };
        for entry in &mut self.windows {
            let Some(x11) = entry.window.x11_surface() else {
                continue;
            };
            let surface = if x11.wl_surface().is_none() {
                x11.wl_surface_id()
                    .and_then(|id| client.object_from_protocol_id(&self.dh, id).ok())
            } else {
                None
            };
            if entry.window.legacy_surface != surface {
                self.space.unmap_elem(&entry.window);
                entry.window.legacy_surface = surface;
                self.epoch += 1;
                if self.focused == Some(entry.id) {
                    self.focused = None;
                }
            }
        }
    }

    fn mapped(window: &SceneWindow) -> bool {
        window.surface().is_some_and(|surface| {
            smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |state| {
                state
                    .buffer()
                    .and_then(|buffer| buffer_dimensions(buffer))
                    .is_some()
            })
            .unwrap_or(false)
        })
    }

    fn parent(&self, entry: &Entry) -> Option<u64> {
        if let Some(toplevel) = entry.window.toplevel() {
            let parent = toplevel.parent()?;
            return self
                .windows
                .iter()
                .find(|e| e.window.surface().as_ref() == Some(&parent))
                .map(|e| e.id);
        }
        let x11 = entry.window.x11_surface()?;
        let parent = x11.is_transient_for()?;
        self.windows
            .iter()
            .find(|e| {
                e.window
                    .x11_surface()
                    .is_some_and(|x| x.window_id() == parent)
            })
            .map(|e| e.id)
    }

    fn root(&self, mut id: u64) -> u64 {
        for _ in 0..self.windows.len() {
            let Some(parent) = self.entry(id).and_then(|entry| self.parent(entry)) else {
                break;
            };
            id = parent;
        }
        id
    }

    fn modal(&self, entry: &Entry) -> bool {
        if self.parent(entry).is_none() {
            return false;
        }
        if let Some(x11) = entry.window.x11_surface() {
            return self
                .x11_properties
                .as_ref()
                .is_some_and(|props| props.is_modal(x11.window_id()));
        }
        entry.window.surface().is_some_and(|surface| {
            with_states(&surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|data| data.lock().ok())
                    .is_some_and(|data| data.modal)
            })
        })
    }

    fn info(&self, entry: &Entry) -> Value {
        let title = if let Some(x11) = entry.window.x11_surface() {
            Some(x11.title())
        } else {
            entry.window.surface().and_then(|surface| {
                with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()?
                        .lock()
                        .ok()?
                        .title
                        .clone()
                })
            })
        };
        json!({"id":entry.id,"title":title.unwrap_or_default(),"focused":self.focused == Some(entry.id),
            "parent":self.parent(entry),"modal":self.modal(entry)})
    }

    fn listed(entry: &Entry) -> bool {
        Self::mapped(&entry.window)
            && !entry
                .window
                .x11_surface()
                .is_some_and(|x| x.is_override_redirect())
    }

    fn refresh(&mut self) {
        self.windows.retain(|entry| entry.window.alive());
        self.associate_legacy_surfaces();
        let chosen = self.selected.filter(|id| self.entry(*id).is_some());
        let chosen = chosen.or_else(|| {
            self.focused
                .and_then(|id| self.entry(id))
                .and_then(|entry| self.parent(entry))
                .filter(|id| self.entry(*id).is_some_and(Self::listed))
        });
        let chosen = chosen.or_else(|| {
            self.windows
                .iter()
                .find(|entry| Self::listed(entry))
                .map(|e| e.id)
        });
        if self.selected != chosen {
            self.selected = chosen;
            self.epoch += 1;
        }

        let family = chosen.map(|id| self.root(id));
        let visible: Vec<_> = self
            .windows
            .iter()
            .filter(|e| {
                let override_redirect = e
                    .window
                    .x11_surface()
                    .is_some_and(|x| x.is_override_redirect());
                let root = if override_redirect {
                    e.popup_owner.map(|owner| self.root(owner))
                } else {
                    Some(self.root(e.id))
                };
                Self::mapped(&e.window) && family.is_some() && root == family
            })
            .map(|e| (e.id, e.window.clone(), self.parent(e).is_some()))
            .collect();

        let hidden: Vec<_> = self
            .space
            .elements()
            .filter(|w| !visible.iter().any(|(_, v, _)| v == *w))
            .cloned()
            .collect();
        if !hidden.is_empty() {
            self.epoch += 1;
        }
        for window in hidden {
            self.space.unmap_elem(&window);
        }
        for (_, window, child) in &visible {
            let loc = if let Some(x11) = window.x11_surface() {
                x11.geometry().loc
            } else if *child {
                let size = window.geometry().size;
                ((SIZE.0 - size.w).max(0) / 2, (SIZE.1 - size.h).max(0) / 2).into()
            } else {
                (0, 0).into()
            };
            if self.space.element_location(window) != Some(loc) {
                let popup = window
                    .x11_surface()
                    .is_some_and(|x| x.is_override_redirect());
                self.space.map_element(window.clone(), loc, false);
                if popup {
                    self.space.raise_element(window, false);
                }
                self.epoch += 1;
            }
        }
        let target = visible
            .iter()
            .rev()
            .find(|(id, _, child)| *child && self.entry(*id).is_some_and(|entry| self.modal(entry)))
            .map(|(id, _, _)| *id)
            .or(chosen);
        self.focus(target);
        self.space.refresh();
        self.popups.cleanup();
    }

    fn focus(&mut self, target: Option<u64>) {
        if self.focused == target {
            return;
        }
        let serial = SERIAL_COUNTER.next_serial();
        let time = self.start.elapsed().as_millis() as u32;
        let keyboard = self.seat.get_keyboard().expect("virtual keyboard");
        let pointer = self.seat.get_pointer().expect("virtual pointer");
        if keyboard.is_grabbed() {
            keyboard.unset_grab(self);
        }
        if pointer.is_grabbed() {
            pointer.unset_grab(self, serial, time);
        }
        self.epoch += 1;
        self.focused = target;
        let surface = target.and_then(|id| self.entry(id)).and_then(|entry| {
            let surface = entry.window.surface()?;
            Some(match entry.window.x11_surface() {
                Some(x11) => KeyboardFocus::X11(Box::new(x11.clone()), surface),
                None => KeyboardFocus::Wayland(surface),
            })
        });
        keyboard.set_focus(self, surface, serial);
        for entry in &self.windows {
            if let Some(toplevel) = entry.window.toplevel() {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                if toplevel.is_initial_configure_sent() {
                    toplevel.with_pending_state(|state| {
                        if Some(entry.id) == target {
                            state.states.set(State::Activated);
                        } else {
                            state.states.unset(State::Activated);
                        }
                    });
                    toplevel.send_pending_configure();
                }
            }
        }
        pointer.motion(
            self,
            None,
            &MotionEvent {
                location: pointer.current_location(),
                serial,
                time,
            },
        );
        pointer.frame(self);
        if let Some(window) = target
            .and_then(|id| self.entry(id))
            .map(|e| e.window.clone())
        {
            self.space.raise_element(&window, false);
            if let (Some(xwm), Some(surface)) = (&mut self.xwm, window.x11_surface()) {
                let _ = xwm.raise_window(surface);
            }
        }
    }

    fn frame(&self) {
        for entry in &self.windows {
            entry.window.send_frame(&self.output, self.start.elapsed());
        }
    }

    fn area(&self, id: u64) -> Result<Rectangle<i32, Logical>> {
        let entry = self.entry(id).ok_or("unknown window")?;
        let location = self
            .space
            .element_location(&entry.window)
            .ok_or("activate the window first")?;
        let mut area = entry.window.bbox_with_popups();
        area.loc += location - SpaceElement::geometry(&entry.window).loc;
        for popup in self
            .windows
            .iter()
            .filter(|popup| popup.popup_owner == Some(entry.id))
        {
            if let Some(location) = self.space.element_location(&popup.window) {
                let mut bounds = popup.window.bbox_with_popups();
                bounds.loc += location - SpaceElement::geometry(&popup.window).loc;
                area = area.merge(bounds);
            }
        }
        area.intersection(Rectangle::from_size(SIZE.into()))
            .ok_or_else(|| "window is outside the virtual output".into())
    }

    fn screenshot(
        &mut self,
        window: u64,
        since: Option<u64>,
        crop: Option<[i32; 4]>,
    ) -> Result<Value> {
        let window_area = self.area(window)?;
        let area = match crop {
            Some([x, y, w, h])
                if x >= 0
                    && y >= 0
                    && w > 0
                    && h > 0
                    && x <= window_area.size.w
                    && y <= window_area.size.h
                    && w <= window_area.size.w - x
                    && h <= window_area.size.h - y =>
            {
                Rectangle::new(
                    (window_area.loc.x + x, window_area.loc.y + y).into(),
                    (w, h).into(),
                )
            }
            Some(_) => return Err("crop is outside the window image".into()),
            None => window_area,
        };
        let pixels = self.renderer.capture(&self.output, &self.space, area)?;
        let mut hasher = DefaultHasher::new();
        pixels.hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(previous) =
            since.and_then(|id| self.captures.iter().find(|capture| capture.id == id))
            && previous.window == window
            && previous.area == area
            && previous.epoch == self.epoch
            && previous.hash == hash
        {
            return Ok(json!({"unchanged":true,"image":previous.id,"focus":self.focused}));
        }
        let id = self.next_image;
        self.next_image += 1;
        let path = self.capture_dir.join(format!("{id}.png"));
        let mut encoder =
            png::Encoder::new(File::create(&path)?, area.size.w as u32, area.size.h as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        if self.captures.len() == 64 {
            self.captures.pop_front();
        }
        self.captures.push_back(Capture {
            id,
            window,
            area,
            window_area,
            epoch: self.epoch,
            hash,
        });
        Ok(
            json!({"image":id,"path":path,"width":area.size.w,"height":area.size.h,"focus":self.focused}),
        )
    }

    fn click(&mut self, image: u64, x: i32, y: i32) -> Result<Value> {
        let capture = self
            .captures
            .iter()
            .find(|capture| capture.id == image)
            .ok_or("unknown image")?;
        if capture.epoch != self.epoch || self.area(capture.window)? != capture.window_area {
            return Err("stale image; take another screenshot".into());
        }
        if self.focused != Some(capture.window) {
            return Err("capture the focused window before clicking".into());
        }
        if x < 0 || y < 0 || x >= capture.area.size.w || y >= capture.area.size.h {
            return Err("click is outside the image".into());
        }
        let position = (capture.area.loc + smithay::utils::Point::from((x, y))).to_f64();
        let focus = self.space.elements().rev().find_map(|window| {
            let location =
                self.space.element_location(window)? - SpaceElement::geometry(window).loc;
            window
                .surface_under(position - location.to_f64())
                .map(|(surface, offset)| (surface, (offset + location).to_f64()))
        });
        let pointer = self.seat.get_pointer().unwrap();
        if focus.is_none() && !pointer.is_grabbed() {
            return Err("no input surface at this position".into());
        }
        let time = self.start.elapsed().as_millis() as u32;
        pointer.motion(
            self,
            focus,
            &MotionEvent {
                location: position,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
        // XWayland must process the motion before the button event. Keep the
        // button pressed across a client frame for immediate-mode toolkits.
        self.click_step = Some((
            Instant::now() + Duration::from_millis(16),
            ButtonState::Pressed,
            self.epoch,
        ));
        self.log(json!({"click":[x,y],"image":image}));
        Ok(json!({"sent":true}))
    }

    fn left_button(&mut self, state: ButtonState) {
        let pointer = self.seat.get_pointer().expect("virtual pointer");
        pointer.button(
            self,
            &ButtonEvent {
                serial: SERIAL_COUNTER.next_serial(),
                time: self.start.elapsed().as_millis() as u32,
                button: 0x110,
                state,
            },
        );
        pointer.frame(self);
    }

    fn key(&mut self, window: u64, code: u32, modifiers: Vec<u32>) -> Result<Value> {
        if self.focused != Some(window) {
            return Err("key target does not have focus".into());
        }
        if !(1..=247).contains(&code) {
            return Err("expected an evdev key code from 1 to 247".into());
        }
        for (index, modifier) in modifiers.iter().enumerate() {
            if !matches!(modifier, 29 | 42 | 54 | 56 | 97 | 100 | 125 | 126)
                || *modifier == code
                || modifiers[..index].contains(modifier)
            {
                return Err(
                    "expected distinct evdev Shift, Ctrl, Alt or Meta modifier codes".into(),
                );
            }
        }
        let keyboard = self.seat.get_keyboard().expect("virtual keyboard");
        let time = self.start.elapsed().as_millis() as u32;
        let chord = modifiers.iter().copied().chain([code]);
        for (code, state) in chord
            .clone()
            .map(|code| (code, KeyState::Pressed))
            .chain(chord.rev().map(|code| (code, KeyState::Released)))
        {
            keyboard.input::<(), _>(
                self,
                (code + 8).into(),
                state,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        }
        self.log(json!({"key":code,"modifiers":modifiers,"window":window}));
        Ok(json!({"sent":true}))
    }

    fn log(&self, mut value: Value) {
        use std::io::Write;
        value["ms"] = json!(self.start.elapsed().as_millis());
        if let Ok(mut file) = File::options()
            .append(true)
            .create(true)
            .open(self.capture_dir.join("actions.jsonl"))
        {
            let _ = writeln!(file, "{value}");
        }
    }

    fn request(&mut self, request: Request) -> Result<Value> {
        match request {
            Request::Windows => Ok(json!(
                self.windows
                    .iter()
                    .filter(|entry| Self::listed(entry))
                    .map(|entry| self.info(entry))
                    .collect::<Vec<_>>()
            )),
            Request::Activate { window } => {
                if !self.entry(window).is_some_and(Self::listed) {
                    return Err("unknown or unmapped window".into());
                }
                if self.selected != Some(window) {
                    self.epoch += 1;
                }
                self.selected = Some(window);
                self.refresh();
                Ok(json!({"focus":self.focused}))
            }
            Request::Maximize { window } | Request::Restore { window } => {
                let entry = self
                    .entry(window)
                    .filter(|entry| Self::listed(entry))
                    .ok_or("unknown or unmapped window")?;
                let target = entry.window.clone();
                self.set_maximized(&target, matches!(request, Request::Maximize { .. }));
                Ok(json!({"sent":true}))
            }
            Request::Screenshot {
                window,
                since,
                crop,
            } => self.screenshot(window, since, crop),
            Request::Click { image, x, y } => self.click(image, x, y),
            Request::Key {
                window,
                code,
                modifiers,
            } => self.key(window, code, modifiers),
            Request::Stop => {
                self.stopped = true;
                Ok(json!({"stopped":true}))
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn run(
    socket: &Path,
    backend: Backend,
    mut apps: Vec<Vec<OsString>>,
    mut x11: Vec<Vec<OsString>>,
) -> Result<()> {
    let dir = tempfile::Builder::new().prefix("rscu-").tempdir()?;
    let wayland_path = dir.path().join("wayland");
    let listener = ListeningSocket::bind_absolute(wayland_path.clone())?;
    let control = UnixListener::bind(socket)?;
    let socket_guard = SocketGuard(socket.to_path_buf());
    control.set_nonblocking(true)?;
    std::fs::set_permissions(socket, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let capture_dir = dir.path().to_path_buf();
    let mut display = Display::<Session>::new()?;
    let mut event_loop = EventLoop::<Session>::try_new()?;
    let mut state = Session::new(display.handle(), capture_dir, backend)?;
    for app in apps.drain(..) {
        state.children.push(launch(&app, &wayland_path, None)?);
    }
    let mut xserver = if x11.is_empty() {
        None
    } else {
        let (server, client) = XWayland::spawn(
            &state.dh,
            Some(free_x11_display()?),
            std::iter::empty::<(&str, &str)>(),
            true,
            Stdio::null(),
            Stdio::inherit(),
            |_| (),
        )?;
        state.xwayland_client = Some(client);
        Some(server)
    };
    let xwayland_deadline = Instant::now() + Duration::from_secs(15);
    eprintln!("Wayland socket: {}", wayland_path.display());
    eprintln!("Control socket: {}", socket.display());
    let mut click_reply: Option<(UnixStream, Value)> = None;
    let mut stop_reply = None;
    while !state.stopped {
        event_loop.dispatch(Duration::from_millis(16), &mut state)?;
        while let Some(client) = listener.accept()? {
            state
                .dh
                .insert_client(client, Arc::new(crate::protocol::WaylandClient::default()))?;
        }
        display.dispatch_clients(&mut state)?;
        if state.xwm.is_none()
            && let Some(server) = &mut xserver
        {
            if let Some(x11_socket) = server.take_socket()? {
                let display_number = server.display_number();
                let client = state
                    .xwayland_client
                    .clone()
                    .expect("registered XWayland client");
                eprintln!("XWayland: :{display_number}");
                state.xwm = Some(X11Wm::start_wm(event_loop.handle(), x11_socket, client)?);
                state.x11_properties = Some(X11Properties::connect(display_number)?);
                for app in x11.drain(..) {
                    state
                        .children
                        .push(launch(&app, &wayland_path, Some(display_number))?);
                }
            } else if Instant::now() >= xwayland_deadline {
                return Err("timed out starting XWayland".into());
            }
        }

        state.refresh();
        if let Some((deadline, button, epoch)) = state.click_step
            && Instant::now() >= deadline
        {
            state.click_step = None;
            if button == ButtonState::Pressed && state.epoch != epoch {
                if let Some((_, response)) = &mut click_reply {
                    *response = json!({"error":"target changed during pointer motion; take another screenshot"});
                }
            } else {
                state.left_button(button);
                if button == ButtonState::Pressed {
                    state.click_step = Some((
                        Instant::now() + Duration::from_millis(45),
                        ButtonState::Released,
                        epoch,
                    ));
                }
            }
        }
        if state.click_step.is_none()
            && let Some((stream, response)) = click_reply.take()
        {
            let _ = serde_json::to_writer(stream, &response);
        }
        if click_reply.is_none()
            && let Ok((stream, _)) = control.accept()
        {
            control::configure(&stream)?;
            let response = control::read_request(&stream)
                .map_err(Into::into)
                .and_then(|request| state.request(request))
                .unwrap_or_else(|error| json!({"error":error.to_string()}));
            if state.stopped {
                stop_reply = Some((stream, response));
            } else if state.click_step.is_some() {
                click_reply = Some((stream, response));
            } else {
                let _ = serde_json::to_writer(stream, &response);
            }
        }
        state
            .children
            .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
        state.frame();
        display.flush_clients()?;
    }
    drop(state);
    drop(xserver);
    drop(event_loop);
    drop(display);
    drop(control);
    drop(socket_guard);
    drop(listener);
    dir.close()?;
    if let Some((stream, response)) = stop_reply {
        let _ = serde_json::to_writer(stream, &response);
    }
    Ok(())
}

fn free_x11_display() -> Result<u32> {
    // Smithay can reuse a display with no lock file. Desktop servers can
    // occupy only the abstract socket, so also check both socket namespaces.
    let unix_sockets = std::fs::read_to_string("/proc/net/unix")?;
    (64..128)
        .find(|number| {
            let path = format!("/tmp/.X11-unix/X{number}");
            let lock = format!("/tmp/.X{number}-lock");
            let abstract_path = format!("@{path}");
            !Path::new(&lock).exists()
                && !Path::new(&path).exists()
                && !unix_sockets.lines().any(|line| {
                    line.split_whitespace()
                        .last()
                        .is_some_and(|socket| socket == path || socket == abstract_path)
                })
        })
        .ok_or_else(|| "no free dedicated X11 display in :64 through :127".into())
}

fn launch(args: &[OsString], wayland: &Path, x11: Option<u32>) -> io::Result<Child> {
    let mut command = Command::new(&args[0]);
    command
        .args(&args[1..])
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env("WAYLAND_DISPLAY", wayland);
    if let Some(display) = x11 {
        command
            .env_remove("WAYLAND_DISPLAY")
            .env("DISPLAY", format!(":{display}"));
    }
    command.spawn()
}

struct SocketGuard(PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

struct X11Properties {
    connection: x11rb::rust_connection::RustConnection,
    state: u32,
    modal: u32,
}

impl X11Properties {
    fn connect(display: u32) -> Result<Self> {
        use x11rb::protocol::xproto::ConnectionExt;
        let (connection, _) = x11rb::connect(Some(&format!(":{display}")))?;
        let state = connection
            .intern_atom(false, b"_NET_WM_STATE")?
            .reply()?
            .atom;
        let modal = connection
            .intern_atom(false, b"_NET_WM_STATE_MODAL")?
            .reply()?
            .atom;
        Ok(Self {
            connection,
            state,
            modal,
        })
    }

    fn same_client(&self, left: u32, right: u32) -> bool {
        use x11rb::connection::Connection;
        let client_mask = !self.connection.setup().resource_id_mask;
        left & client_mask == right & client_mask
    }

    fn is_modal(&self, window: u32) -> bool {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
        self.connection
            .get_property(false, window, self.state, AtomEnum::ATOM, 0, 64)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| {
                reply
                    .value32()
                    .map(|mut values| values.any(|atom| atom == self.modal))
            })
            .unwrap_or(false)
    }
}
