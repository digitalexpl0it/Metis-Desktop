use std::borrow::Cow;

use smithay::{
    backend::input::{InputTime, KeyState},
    desktop::{LayerSurface, PopupKind, Window},
    input::{
        Seat,
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Serial},
    wayland::seat::WaylandFocus,
};

use crate::state::MetisState;

#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardFocusTarget {
    Window(Window),
    LayerSurface(LayerSurface),
    Popup(PopupKind),
    /// `ext-session-lock-v1` surface from a third-party locker.
    LockSurface(WlSurface),
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            Self::Window(w) => w.alive(),
            Self::LayerSurface(l) => l.alive(),
            Self::Popup(p) => p.alive(),
            Self::LockSurface(s) => s.alive(),
        }
    }
}

impl KeyboardFocusTarget {
    /// Run `f` against the underlying keyboard target surface. Returns `None`
    /// only for a `Window` that has no associated `wl_surface` yet (e.g. an
    /// unmapped X11 surface), in which case the keyboard event is simply dropped.
    fn with_keyboard_surface<R>(
        &self,
        f: impl FnOnce(&dyn KeyboardTarget<MetisState>) -> R,
    ) -> Option<R> {
        match self {
            Self::Window(w) => match w.toplevel() {
                // Wayland toplevel: deliver to its stable `&WlSurface`.
                Some(toplevel) => Some(f(toplevel.wl_surface())),
                // XWayland: deliver to the `X11Surface` itself — NOT its raw
                // `wl_surface`. `X11Surface`'s own `KeyboardTarget` impl performs
                // the X-specific focus handling the client needs to accept keys:
                // it calls `XSetInputFocus` (and sends `WM_TAKE_FOCUS` for
                // active-input clients) before forwarding the wl_keyboard events.
                // Delivering to the bare `wl_surface` skips all of that, so the X
                // client never gains input focus — keyboard (Esc, WASD, …) is
                // ignored while pointer *clicks* still work (they don't need
                // focus). It also buffers enter/key in `pending_enter` until the
                // surface associates, natively handling the map-before-surface
                // race for games that map and get focus before XWayland links
                // their `wl_surface`.
                None => w.x11_surface().map(|x11| f(x11)),
            },
            Self::LayerSurface(l) => Some(f(l.wl_surface())),
            Self::Popup(p) => Some(f(p.wl_surface())),
            Self::LockSurface(s) => Some(f(s)),
        }
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            Self::Window(w) => w.wl_surface(),
            Self::LayerSurface(l) => Some(Cow::Borrowed(l.wl_surface())),
            Self::Popup(p) => Some(Cow::Borrowed(p.wl_surface())),
            Self::LockSurface(s) => Some(Cow::Borrowed(s)),
        }
    }
}

impl KeyboardTarget<MetisState> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<MetisState>,
        data: &mut MetisState,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        self.with_keyboard_surface(|target| target.enter(seat, data, keys, serial));
    }

    fn leave(&self, seat: &Seat<MetisState>, data: &mut MetisState, serial: Serial) {
        self.with_keyboard_surface(|target| target.leave(seat, data, serial));
    }

    fn key(
        &self,
        seat: &Seat<MetisState>,
        data: &mut MetisState,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        self.with_keyboard_surface(|target| target.key(seat, data, key, state, serial, time));
    }

    fn modifiers(
        &self,
        seat: &Seat<MetisState>,
        data: &mut MetisState,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        self.with_keyboard_surface(|target| target.modifiers(seat, data, modifiers, serial));
    }
}

impl From<Window> for KeyboardFocusTarget {
    fn from(w: Window) -> Self {
        Self::Window(w)
    }
}

impl From<LayerSurface> for KeyboardFocusTarget {
    fn from(l: LayerSurface) -> Self {
        Self::LayerSurface(l)
    }
}

impl From<PopupKind> for KeyboardFocusTarget {
    fn from(p: PopupKind) -> Self {
        Self::Popup(p)
    }
}

impl From<KeyboardFocusTarget> for WlSurface {
    fn from(target: KeyboardFocusTarget) -> Self {
        match target {
            KeyboardFocusTarget::Window(w) => w
                .wl_surface()
                .map(|surface| surface.into_owned())
                .unwrap_or_else(|| w.toplevel().unwrap().wl_surface().clone()),
            KeyboardFocusTarget::LayerSurface(l) => l.wl_surface().clone(),
            KeyboardFocusTarget::Popup(p) => p.wl_surface().clone(),
            KeyboardFocusTarget::LockSurface(s) => s,
        }
    }
}
