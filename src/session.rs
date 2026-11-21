use crate::{
    control::{self, Request},
    focus::KeyboardFocus,
    render::{Backend, Renderer},
};
use serde_json::{Value, json};
use smithay::{
    backend::{
        input::{ButtonState, KeyState},
        renderer::buffer_dimensions,
    },
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{
        Seat, SeatState,
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::EventLoop,
        wayland_server::{
            Display, DisplayHandle, ListeningSocket, protocol::wl_surface::WlSurface,
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
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};
use std::{
    collections::VecDeque,
    ffi::OsString,
    fs::File,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub const SIZE: (i32, i32) = (1280, 800);

pub struct Entry {
    pub id: u64,
    pub window: Window,
    pub restore: Option<Rectangle<i32, Logical>>,
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
    pub space: Space<Window>,
    pub xwm: Option<X11Wm>,
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
    startup_error: Option<String>,
    children: Vec<Child>,
}

impl Session {
    fn new(dh: DisplayHandle, capture_dir: PathBuf, backend: Backend) -> Result<Self> {
        let compositor = CompositorState::new::<Self>(&dh);
        let xdg = XdgShellState::new::<Self>(&dh);
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
            startup_error: None,
            children: Vec::new(),
        })
    }

    pub fn insert(&mut self, window: Window) {
        self.windows.push(Entry {
            id: self.next_window,
            window,
            restore: None,
        });
        self.next_window += 1;
    }

    fn entry(&self, id: u64) -> Option<&Entry> {
        self.windows.iter().find(|entry| entry.id == id)
    }

    pub fn focus_matches(&self, surface: &WlSurface) -> bool {
        self.focused
            .and_then(|id| self.entry(id))
            .and_then(|e| Self::surface(&e.window))
            .as_ref()
            == Some(surface)
    }

    pub fn surface(window: &Window) -> Option<WlSurface> {
        if let Some(toplevel) = window.toplevel() {
            Some(toplevel.wl_surface().clone())
        } else {
            window.x11_surface()?.wl_surface()
        }
    }

    fn mapped(window: &Window) -> bool {
        Self::surface(window).is_some_and(|surface| {
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
                .find(|e| Self::surface(&e.window).as_ref() == Some(&parent))
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
        Self::surface(&entry.window).is_some_and(|surface| {
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
            Self::surface(&entry.window).and_then(|surface| {
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
        let chosen = self
            .selected
            .filter(|id| self.entry(*id).is_some_and(Self::listed));
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
                Self::mapped(&e.window)
                    && (Some(self.root(e.id)) == family
                        || (e
                            .window
                            .x11_surface()
                            .is_some_and(|x| x.is_override_redirect())
                            && family
                                .and_then(|id| self.entry(id))
                                .and_then(|root| root.window.x11_surface())
                                .is_some_and(|x| {
                                    e.window
                                        .x11_surface()
                                        .unwrap()
                                        .pid()
                                        .zip(x.pid())
                                        .is_some_and(|(a, b)| a == b)
                                })))
            })
            .map(|e| (e.id, e.window.clone(), self.parent(e).is_some()))
            .collect();

        let hidden: Vec<_> = self
            .space
            .elements()
            .filter(|w| !visible.iter().any(|(_, v, _)| v == *w))
            .cloned()
            .collect();
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
                self.space.map_element(window.clone(), loc, false);
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
            let surface = Self::surface(&entry.window)?;
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
            } else if let Some(x11) = entry.window.x11_surface() {
                let _ = x11.set_activated(Some(entry.id) == target);
            }
        }
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
            entry.window.send_frame(
                &self.output,
                self.start.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(self.output.clone()),
            );
        }
    }

    fn area(&self, id: u64) -> Result<Rectangle<i32, Logical>> {
        let entry = self.entry(id).ok_or("unknown window")?;
        let location = self
            .space
            .element_location(&entry.window)
            .ok_or("activate the window first")?;
        let mut area = entry.window.bbox_with_popups();
        area.loc += location - entry.window.geometry().loc;
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
            let location = self.space.element_location(window)? - window.geometry().loc;
            window
                .surface_under(position - location.to_f64(), WindowSurfaceType::ALL)
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
        for state in [ButtonState::Pressed, ButtonState::Released] {
            pointer.button(
                self,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    button: 0x110,
                    state,
                },
            );
            pointer.frame(self);
        }
        self.log(json!({"click":[x,y],"image":image}));
        Ok(json!({"sent":true}))
    }

    fn key(&mut self, window: u64, code: u32) -> Result<Value> {
        if self.focused != Some(window) {
            return Err("key target does not have focus".into());
        }
        if !(1..=247).contains(&code) {
            return Err("expected an evdev key code from 1 to 247".into());
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        let time = self.start.elapsed().as_millis() as u32;
        for state in [KeyState::Pressed, KeyState::Released] {
            keyboard.input::<(), _>(
                self,
                (code + 8).into(),
                state,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        }
        self.log(json!({"key":code,"window":window}));
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
            Request::Screenshot {
                window,
                since,
                crop,
            } => self.screenshot(window, since, crop),
            Request::Click { image, x, y } => self.click(image, x, y),
            Request::Key { window, code } => self.key(window, code),
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
    let _socket_guard = SocketGuard(socket.to_path_buf());
    control.set_nonblocking(true)?;
    std::fs::set_permissions(socket, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let capture_dir = dir.path().to_path_buf();
    let mut display = Display::<Session>::new()?;
    let mut event_loop = EventLoop::<Session>::try_new()?;
    let mut state = Session::new(display.handle(), capture_dir, backend)?;
    for app in apps.drain(..) {
        state.children.push(launch(&app, &wayland_path, None)?);
    }
    if !x11.is_empty() {
        let (server, xwayland_client) = XWayland::spawn(
            &state.dh,
            None,
            std::iter::empty::<(&str, &str)>(),
            false,
            Stdio::null(),
            Stdio::inherit(),
            |_| (),
        )?;
        let handle = event_loop.handle();
        let xwm_handle = handle.clone();
        let child_wayland = wayland_path.clone();
        handle
            .insert_source(server, move |event, _, state| match event {
                XWaylandEvent::Ready {
                    x11_socket,
                    display_number,
                } => {
                    let result = (|| -> Result<()> {
                        state.xwm = Some(X11Wm::start_wm(
                            xwm_handle.clone(),
                            x11_socket,
                            xwayland_client.clone(),
                        )?);
                        state.x11_properties = Some(X11Properties::connect(display_number)?);
                        for app in x11.drain(..) {
                            state.children.push(launch(
                                &app,
                                &child_wayland,
                                Some(display_number),
                            )?);
                        }
                        Ok(())
                    })();
                    if let Err(error) = result {
                        state.startup_error = Some(format!("XWayland: {error}"));
                        state.stopped = true;
                    }
                }
                XWaylandEvent::Error => {
                    state.startup_error = Some("XWayland failed to start".into());
                    state.stopped = true;
                }
            })
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    eprintln!("Wayland socket: {}", wayland_path.display());
    eprintln!("Control socket: {}", socket.display());
    while !state.stopped {
        event_loop.dispatch(Duration::from_millis(16), &mut state)?;
        while let Some(client) = listener.accept()? {
            state
                .dh
                .insert_client(client, Arc::new(crate::protocol::WaylandClient::default()))?;
        }
        display.dispatch_clients(&mut state)?;
        state.refresh();
        if let Ok((stream, _)) = control.accept() {
            control::configure(&stream)?;
            let response = control::read_request(&stream)
                .map_err(Into::into)
                .and_then(|request| state.request(request))
                .unwrap_or_else(|error| json!({"error":error.to_string()}));
            let _ = serde_json::to_writer(stream, &response);
        }
        state
            .children
            .retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
        state.frame();
        display.flush_clients()?;
    }
    if let Some(error) = state.startup_error.take() {
        return Err(error.into());
    }
    Ok(())
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
