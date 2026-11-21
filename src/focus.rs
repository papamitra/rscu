use crate::session::Session;
use smithay::{
    backend::input::KeyState,
    desktop::PopupKind,
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
    xwayland::X11Surface,
};
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq)]
pub enum KeyboardFocus {
    Wayland(WlSurface),
    X11(Box<X11Surface>, WlSurface),
}

impl From<PopupKind> for KeyboardFocus {
    fn from(popup: PopupKind) -> Self {
        Self::Wayland(popup.into())
    }
}

impl From<KeyboardFocus> for WlSurface {
    fn from(focus: KeyboardFocus) -> Self {
        match focus {
            KeyboardFocus::Wayland(surface) | KeyboardFocus::X11(_, surface) => surface,
        }
    }
}

impl WaylandFocus for KeyboardFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Wayland(surface) | Self::X11(_, surface) => Some(Cow::Borrowed(surface)),
        }
    }
}

impl IsAlive for KeyboardFocus {
    fn alive(&self) -> bool {
        match self {
            Self::Wayland(surface) => surface.alive(),
            Self::X11(window, surface) => window.alive() && surface.alive(),
        }
    }
}

impl KeyboardTarget<Session> for KeyboardFocus {
    fn enter(
        &self,
        seat: &Seat<Session>,
        state: &mut Session,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::enter(surface, seat, state, keys, serial),
            Self::X11(window, surface) if window.wl_surface().is_none() => {
                window.enter(seat, state, Vec::new(), serial);
                KeyboardTarget::enter(surface, seat, state, keys, serial);
            }
            Self::X11(window, _) => window.enter(seat, state, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<Session>, state: &mut Session, serial: Serial) {
        match self {
            Self::Wayland(surface) => KeyboardTarget::leave(surface, seat, state, serial),
            Self::X11(window, surface) => {
                window.leave(seat, state, serial);
                if window.wl_surface().is_none() {
                    KeyboardTarget::leave(surface, seat, state, serial);
                }
            }
        }
    }

    fn key(
        &self,
        seat: &Seat<Session>,
        state: &mut Session,
        key: KeysymHandle<'_>,
        key_state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            Self::Wayland(surface) => surface.key(seat, state, key, key_state, serial, time),
            Self::X11(window, surface) if window.wl_surface().is_none() => {
                surface.key(seat, state, key, key_state, serial, time)
            }
            Self::X11(window, _) => window.key(seat, state, key, key_state, serial, time),
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<Session>,
        state: &mut Session,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            Self::Wayland(surface) => surface.modifiers(seat, state, modifiers, serial),
            Self::X11(window, surface) if window.wl_surface().is_none() => {
                surface.modifiers(seat, state, modifiers, serial)
            }
            Self::X11(window, _) => window.modifiers(seat, state, modifiers, serial),
        }
    }
}
