use crate::focus::KeyboardFocus;
use crate::session::{SIZE, Session};
use smithay::wayland::seat::WaylandFocus;
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_dialog, delegate_xdg_shell, delegate_xwayland_shell,
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
        find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{CursorImageStatus, Focus},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::State,
        wayland_server::{
            Client, Resource,
            backend::ClientData,
            protocol::{wl_buffer::WlBuffer, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Rectangle},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{
        X11Surface, X11Wm, XWaylandClientData, XwmHandler,
        xwm::{Reorder, ResizeEdge, X11Window, XwmId},
    },
};

#[derive(Default)]
pub struct WaylandClient(CompositorClientState);
impl ClientData for WaylandClient {}

impl CompositorHandler for Session {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(data) = client.get_data::<XWaylandClientData>() {
            return &data.compositor_state;
        }
        &client
            .get_data::<WaylandClient>()
            .expect("registered Wayland client")
            .0
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(entry) = self
                .windows
                .iter()
                .find(|e| Session::surface(&e.window).as_ref() == Some(&root))
            {
                entry.window.on_commit();
                if let Some(toplevel) = entry.window.toplevel()
                    && !toplevel.is_initial_configure_sent()
                {
                    toplevel.send_configure();
                }
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popups.find_popup(surface)
            && !popup.is_initial_configure_sent()
        {
            let _ = popup.send_configure();
        }
    }
}

impl BufferHandler for Session {
    fn buffer_destroyed(&mut self, _: &WlBuffer) {}
}
impl ShmHandler for Session {
    fn shm_state(&self) -> &ShmState {
        &self.shm
    }
}
impl OutputHandler for Session {}
impl SeatHandler for Session {
    type KeyboardFocus = KeyboardFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seats
    }
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
    fn focus_changed(&mut self, seat: &Seat<Self>, focus: Option<&KeyboardFocus>) {
        let client = focus
            .and_then(WaylandFocus::wl_surface)
            .and_then(|surface| self.dh.get_client(surface.id()).ok());
        set_data_device_focus(&self.dh, seat, client);
    }
}
impl SelectionHandler for Session {
    type SelectionUserData = ();
}
impl DataDeviceHandler for Session {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device
    }
}
impl ClientDndGrabHandler for Session {}
impl ServerDndGrabHandler for Session {}

impl XdgShellHandler for Session {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.insert(Window::new_wayland_window(surface));
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.position_popup(&surface, positioner);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        self.position_popup(&surface, positioner);
        surface.send_repositioned(token);
    }

    fn grab(
        &mut self,
        surface: PopupSurface,
        seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        serial: smithay::utils::Serial,
    ) {
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        if seat != self.seat || !self.focus_matches(&root) {
            let _ = smithay::desktop::PopupManager::dismiss_popup(&root, &kind);
            return;
        }
        let Ok(mut grab) =
            self.popups
                .grab_popup(KeyboardFocus::Wayland(root), kind, &seat, serial)
        else {
            return;
        };
        let keyboard = seat.get_keyboard().expect("virtual keyboard");
        let pointer = seat.get_pointer().expect("virtual pointer");
        let previous = grab.previous_serial().unwrap_or(serial);
        if (keyboard.is_grabbed() && !keyboard.has_grab(previous) && !keyboard.has_grab(serial))
            || (pointer.is_grabbed() && !pointer.has_grab(previous) && !pointer.has_grab(serial))
        {
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        keyboard.set_focus(self, grab.current_grab(), serial);
        keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.resize_wayland(surface, State::Maximized, true);
    }
    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.resize_wayland(surface, State::Maximized, false);
    }
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        self.resize_wayland(surface, State::Fullscreen, true);
    }
    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.resize_wayland(surface, State::Fullscreen, false);
    }
}

impl Session {
    fn position_popup(&self, popup: &PopupSurface, positioner: PositionerState) {
        let kind = PopupKind::Xdg(popup.clone());
        let origin = find_popup_root_surface(&kind)
            .ok()
            .and_then(|surface| {
                self.windows
                    .iter()
                    .find(|e| Self::surface(&e.window).as_ref() == Some(&surface))
            })
            .and_then(|entry| self.space.element_location(&entry.window))
            .unwrap_or_default();
        let mut bounds = Rectangle::from_size(SIZE.into());
        bounds.loc -= origin + get_popup_toplevel_coords(&kind);
        popup.with_pending_state(|state| {
            state.positioner = positioner;
            state.geometry = positioner.get_unconstrained_geometry(bounds);
        });
    }

    fn resize_wayland(&mut self, surface: ToplevelSurface, flag: State, enabled: bool) {
        let entry = self
            .windows
            .iter_mut()
            .find(|entry| entry.window.toplevel() == Some(&surface));
        surface.with_pending_state(|state| {
            if enabled {
                if let Some(entry) = entry {
                    entry.restore.get_or_insert(entry.window.geometry());
                }
                state.states.set(flag);
                state.size = Some(SIZE.into());
            } else {
                state.states.unset(flag);
                if !state.states.contains(State::Maximized)
                    && !state.states.contains(State::Fullscreen)
                {
                    state.size = entry
                        .and_then(|entry| entry.restore.take())
                        .map(|rect| rect.size);
                }
            }
        });
        if surface.is_initial_configure_sent() {
            surface.send_pending_configure();
        }
    }

    fn track_x11(&mut self, surface: X11Surface) {
        if !self
            .windows
            .iter()
            .any(|entry| entry.window.x11_surface() == Some(&surface))
        {
            self.insert(Window::new_x11_window(surface));
        }
    }

    fn forget_x11(&mut self, surface: &X11Surface) {
        if let Some(index) = self
            .windows
            .iter()
            .position(|entry| entry.window.x11_surface() == Some(surface))
        {
            let entry = self.windows.remove(index);
            self.space.unmap_elem(&entry.window);
        }
    }

    fn resize_x11(&mut self, surface: X11Surface, enabled: bool, fullscreen: bool) {
        let entry = self
            .windows
            .iter_mut()
            .find(|entry| entry.window.x11_surface() == Some(&surface));
        let rectangle = if enabled {
            if let Some(entry) = entry {
                entry.restore.get_or_insert_with(|| surface.geometry());
            }
            Some(Rectangle::from_size(SIZE.into()))
        } else if (fullscreen && surface.is_maximized()) || (!fullscreen && surface.is_fullscreen())
        {
            None
        } else {
            entry.and_then(|entry| entry.restore.take())
        };
        if fullscreen {
            let _ = surface.set_fullscreen(enabled);
        } else {
            let _ = surface.set_maximized(enabled);
        }
        if let Some(rectangle) = rectangle {
            let _ = surface.configure(rectangle);
        }
    }
}

impl XWaylandShellHandler for Session {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell
    }

    fn surface_associated(&mut self, _: XwmId, _: WlSurface, window: X11Surface) {
        if window.is_mapped() {
            self.track_x11(window);
        }
    }
}

impl XwmHandler for Session {
    fn xwm_state(&mut self, _: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("XWM started")
    }
    fn new_window(&mut self, _: XwmId, _: X11Surface) {}
    fn new_override_redirect_window(&mut self, _: XwmId, _: X11Surface) {}
    fn map_window_request(&mut self, _: XwmId, window: X11Surface) {
        if window.set_mapped(true).is_ok() {
            let mut geometry = window.geometry();
            let parent = window.is_transient_for();
            if parent.is_none() {
                geometry.loc = (0, 0).into();
            } else {
                geometry.loc = (
                    (SIZE.0 - geometry.size.w).max(0) / 2,
                    (SIZE.1 - geometry.size.h).max(0) / 2,
                )
                    .into();
            }
            let _ = window.configure(geometry);
            self.track_x11(window);
        }
    }
    fn mapped_override_redirect_window(&mut self, _: XwmId, window: X11Surface) {
        self.track_x11(window);
    }
    fn unmapped_window(&mut self, _: XwmId, window: X11Surface) {
        self.forget_x11(&window);
    }
    fn destroyed_window(&mut self, _: XwmId, window: X11Surface) {
        self.forget_x11(&window);
    }
    fn configure_request(
        &mut self,
        _: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        _: Option<Reorder>,
    ) {
        let mut rect = window.geometry();
        if !window.is_maximized() && !window.is_fullscreen() {
            rect.loc.x = x.unwrap_or(rect.loc.x);
            rect.loc.y = y.unwrap_or(rect.loc.y);
            rect.size.w = width
                .map(|w| w.clamp(1, i16::MAX as u32) as i32)
                .unwrap_or(rect.size.w);
            rect.size.h = height
                .map(|h| h.clamp(1, i16::MAX as u32) as i32)
                .unwrap_or(rect.size.h);
        }
        let _ = window.configure(rect);
    }
    fn configure_notify(
        &mut self,
        _: XwmId,
        _: X11Surface,
        _: Rectangle<i32, Logical>,
        _: Option<X11Window>,
    ) {
    }
    fn maximize_request(&mut self, _: XwmId, window: X11Surface) {
        self.resize_x11(window, true, false);
    }
    fn unmaximize_request(&mut self, _: XwmId, window: X11Surface) {
        self.resize_x11(window, false, false);
    }
    fn fullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        self.resize_x11(window, true, true);
    }
    fn unfullscreen_request(&mut self, _: XwmId, window: X11Surface) {
        self.resize_x11(window, false, true);
    }
    fn resize_request(&mut self, _: XwmId, _: X11Surface, _: u32, _: ResizeEdge) {}
    fn move_request(&mut self, _: XwmId, _: X11Surface, _: u32) {}
}

delegate_compositor!(Session);
delegate_shm!(Session);
delegate_output!(Session);
delegate_seat!(Session);
delegate_data_device!(Session);
impl smithay::wayland::shell::xdg::dialog::XdgDialogHandler for Session {}
delegate_xdg_dialog!(Session);
delegate_xdg_shell!(Session);
delegate_xwayland_shell!(Session);

impl smithay::wayland::dmabuf::DmabufHandler for Session {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf
    }

    fn dmabuf_imported(
        &mut self,
        _: &smithay::wayland::dmabuf::DmabufGlobal,
        buffer: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        if self.renderer.import_dmabuf(&buffer) {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}
smithay::delegate_dmabuf!(Session);
