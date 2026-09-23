//! libeis server for gnome-remote-desktop ConnectToEIS.
//!
//! Modelled on Mutter's `meta-eis-client.c`: one seat per sender client, devices
//! created on `SEAT_BIND`, torn down before the seat is unreffed. Absolute pointer
//! regions track ScreenCast viewports and refresh when the stream resizes.

use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd as StdOwnedFd, RawFd};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::compositor_remote_input;

type Eis = std::ffi::c_void;
type EisEvent = std::ffi::c_void;
type EisClient = std::ffi::c_void;
type EisSeat = std::ffi::c_void;
type EisDevice = std::ffi::c_void;
type EisRegion = std::ffi::c_void;
type EisKeymap = std::ffi::c_void;

const EIS_EVENT_CLIENT_CONNECT: u32 = 1;
const EIS_EVENT_CLIENT_DISCONNECT: u32 = 2;
const EIS_EVENT_SEAT_BIND: u32 = 3;
const EIS_EVENT_DEVICE_CLOSED: u32 = 4;
const EIS_EVENT_POINTER_MOTION: u32 = 300;
const EIS_EVENT_POINTER_MOTION_ABSOLUTE: u32 = 400;
const EIS_EVENT_BUTTON_BUTTON: u32 = 500;
const EIS_EVENT_SCROLL_DELTA: u32 = 600;
const EIS_EVENT_KEYBOARD_KEY: u32 = 700;

const EIS_DEVICE_CAP_POINTER: u32 = 1 << 0;
const EIS_DEVICE_CAP_POINTER_ABSOLUTE: u32 = 1 << 1;
const EIS_DEVICE_CAP_KEYBOARD: u32 = 1 << 2;
const EIS_DEVICE_CAP_SCROLL: u32 = 1 << 4;
const EIS_DEVICE_CAP_BUTTON: u32 = 1 << 5;

const EIS_KEYMAP_TYPE_XKB: u32 = 1;

/// Screen region advertised to GRD — must match ScreenCast stream parameters.
#[derive(Debug, Clone)]
pub struct Viewport {
    pub mapping_id: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

#[cfg(has_libeis)]
#[link(name = "eis")]
unsafe extern "C" {
    fn eis_new(user_data: *mut std::ffi::c_void) -> *mut Eis;
    fn eis_unref(eis: *mut Eis);
    fn eis_setup_backend_fd(ctx: *mut Eis) -> i32;
    fn eis_backend_fd_add_client(ctx: *mut Eis) -> i32;
    fn eis_get_fd(eis: *mut Eis) -> i32;
    fn eis_dispatch(eis: *mut Eis) -> i32;
    fn eis_get_event(eis: *mut Eis) -> *mut EisEvent;
    fn eis_event_unref(event: *mut EisEvent);
    fn eis_event_get_type(event: *mut EisEvent) -> u32;
    fn eis_event_get_client(event: *mut EisEvent) -> *mut EisClient;
    fn eis_event_get_device(event: *mut EisEvent) -> *mut EisDevice;
    fn eis_client_connect(client: *mut EisClient);
    fn eis_client_is_sender(client: *mut EisClient) -> bool;
    fn eis_client_new_seat(client: *mut EisClient, name: *const i8) -> *mut EisSeat;
    fn eis_seat_configure_capability(seat: *mut EisSeat, cap: u32);
    fn eis_seat_add(seat: *mut EisSeat);
    fn eis_seat_ref(seat: *mut EisSeat) -> *mut EisSeat;
    fn eis_seat_unref(seat: *mut EisSeat);
    fn eis_seat_new_device(seat: *mut EisSeat) -> *mut EisDevice;
    fn eis_device_configure_name(device: *mut EisDevice, name: *const i8);
    fn eis_device_configure_capability(device: *mut EisDevice, cap: u32);
    fn eis_device_add(device: *mut EisDevice);
    fn eis_device_pause(device: *mut EisDevice);
    fn eis_device_remove(device: *mut EisDevice);
    fn eis_device_resume(device: *mut EisDevice);
    fn eis_device_unref(device: *mut EisDevice);
    fn eis_device_new_region(device: *mut EisDevice) -> *mut EisRegion;
    fn eis_device_get_region_at(device: *mut EisDevice, x: f64, y: f64) -> *mut EisRegion;
    fn eis_device_new_keymap(
        device: *mut EisDevice,
        ty: u32,
        fd: i32,
        size: usize,
    ) -> *mut EisKeymap;
    fn eis_region_set_size(region: *mut EisRegion, w: u32, h: u32);
    fn eis_region_set_offset(region: *mut EisRegion, x: u32, y: u32);
    fn eis_region_set_physical_scale(region: *mut EisRegion, scale: f64);
    fn eis_region_set_mapping_id(region: *mut EisRegion, id: *const i8);
    fn eis_region_add(region: *mut EisRegion);
    fn eis_region_unref(region: *mut EisRegion);
    fn eis_region_get_x(region: *mut EisRegion) -> u32;
    fn eis_region_get_y(region: *mut EisRegion) -> u32;
    fn eis_keymap_add(keymap: *mut EisKeymap);
    fn eis_keymap_unref(keymap: *mut EisKeymap);
    fn eis_event_seat_has_capability(event: *mut EisEvent, cap: u32) -> bool;
    fn eis_event_pointer_get_dx(event: *mut EisEvent) -> f64;
    fn eis_event_pointer_get_dy(event: *mut EisEvent) -> f64;
    fn eis_event_pointer_get_absolute_x(event: *mut EisEvent) -> f64;
    fn eis_event_pointer_get_absolute_y(event: *mut EisEvent) -> f64;
    fn eis_event_button_get_button(event: *mut EisEvent) -> u32;
    fn eis_event_button_get_is_press(event: *mut EisEvent) -> bool;
    fn eis_event_scroll_get_dx(event: *mut EisEvent) -> f64;
    fn eis_event_scroll_get_dy(event: *mut EisEvent) -> f64;
    fn eis_event_keyboard_get_key(event: *mut EisEvent) -> u32;
    fn eis_event_keyboard_get_key_is_press(event: *mut EisEvent) -> bool;
}

struct EisServer {
    ctx: usize,
}

unsafe impl Send for EisServer {}

impl EisServer {
    fn new() -> Result<Self, String> {
        #[cfg(has_libeis)]
        {
            let ctx = unsafe { eis_new(std::ptr::null_mut()) };
            if ctx.is_null() {
                return Err("eis_new failed".into());
            }
            let rc = unsafe { eis_setup_backend_fd(ctx) };
            if rc < 0 {
                unsafe { eis_unref(ctx) };
                return Err(format!("eis_setup_backend_fd failed: {rc}"));
            }
            Ok(Self { ctx: ctx as usize })
        }
        #[cfg(not(has_libeis))]
        {
            Err("libeis not available at build time (install libeis-dev)".into())
        }
    }

    fn ctx(&self) -> *mut Eis {
        self.ctx as *mut Eis
    }

    fn client_fd(&self) -> Result<StdOwnedFd, String> {
        #[cfg(has_libeis)]
        {
            let fd = unsafe { eis_backend_fd_add_client(self.ctx()) };
            if fd < 0 {
                return Err(format!("eis_backend_fd_add_client failed: {fd}"));
            }
            Ok(unsafe { StdOwnedFd::from_raw_fd(fd as RawFd) })
        }
        #[cfg(not(has_libeis))]
        {
            let _ = self;
            Err("libeis not available".into())
        }
    }

    fn run_loop(ctx_addr: usize) {
        #[cfg(has_libeis)]
        {
            let ctx = ctx_addr as *mut Eis;
            let fd = unsafe { eis_get_fd(ctx) };
            if fd < 0 {
                tracing::error!("eis_get_fd failed");
                return;
            }
            loop {
                let mut pfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let rc = unsafe { libc::poll(&mut pfd, 1, 500) };
                if rc <= 0 {
                    continue;
                }
                unsafe {
                    eis_dispatch(ctx);
                }
                loop {
                    let ev = unsafe { eis_get_event(ctx) };
                    if ev.is_null() {
                        break;
                    }
                    handle_event(ev);
                    unsafe {
                        eis_event_unref(ev);
                    }
                }
            }
        }
    }
}

impl Drop for EisServer {
    fn drop(&mut self) {
        #[cfg(has_libeis)]
        if self.ctx != 0 {
            unsafe {
                eis_unref(self.ctx());
            }
        }
    }
}

struct ClientState {
    seat: usize,
    rel_pointer: Option<usize>,
    keyboard: Option<usize>,
    abs_pointer: Option<usize>,
}

struct EisHub {
    viewports: HashMap<String, Viewport>,
    client: Option<ClientState>,
}

impl EisHub {
    fn new() -> Self {
        Self {
            viewports: HashMap::new(),
            client: None,
        }
    }
}

static EIS: OnceLock<Arc<Mutex<EisServer>>> = OnceLock::new();
static HUB: OnceLock<Arc<Mutex<EisHub>>> = OnceLock::new();

fn hub() -> Arc<Mutex<EisHub>> {
    Arc::clone(HUB.get_or_init(|| Arc::new(Mutex::new(EisHub::new()))))
}

fn server() -> Result<Arc<Mutex<EisServer>>, String> {
    if let Some(s) = EIS.get() {
        return Ok(Arc::clone(s));
    }
    let _ = hub();
    let srv = EisServer::new()?;
    let ctx_addr = srv.ctx;
    thread::Builder::new()
        .name("metis-eis".into())
        .spawn(move || EisServer::run_loop(ctx_addr))
        .map_err(|e| format!("spawn eis thread: {e}"))?;
    let arc = Arc::new(Mutex::new(srv));
    let _ = EIS.set(Arc::clone(&arc));
    Ok(arc)
}

pub fn client_fd() -> Result<StdOwnedFd, String> {
    server()?
        .lock()
        .map_err(|_| "eis lock".to_string())?
        .client_fd()
}

fn apply_viewport(hub: &mut EisHub, viewport: Viewport) {
    if viewport.width == 0 || viewport.height == 0 {
        tracing::warn!(
            mapping_id = %viewport.mapping_id,
            "EIS: ignoring viewport with zero size"
        );
        return;
    }
    tracing::info!(
        mapping_id = %viewport.mapping_id,
        width = viewport.width,
        height = viewport.height,
        "EIS: registering viewport"
    );
    hub.viewports.insert(viewport.mapping_id.clone(), viewport);
    refresh_absolute_devices(hub);
}

/// Register a ScreenCast viewport (call when the stream object is created).
pub fn register_viewport(viewport: Viewport) {
    let hub_arc = hub();
    let Ok(mut hub) = hub_arc.lock() else {
        return;
    };
    apply_viewport(&mut hub, viewport);
}

/// Update viewport geometry when the stream (re)starts or the monitor layout changes.
pub fn update_viewport(viewport: Viewport) {
    register_viewport(viewport);
}

/// Remove a ScreenCast viewport when its stream stops.
pub fn unregister_viewport(mapping_id: &str) {
    let hub_arc = hub();
    let Ok(mut hub) = hub_arc.lock() else {
        return;
    };
    hub.viewports.remove(mapping_id);
    refresh_absolute_devices(&mut hub);
}

#[cfg(has_libeis)]
unsafe fn remove_device(device: *mut EisDevice) {
    unsafe {
        if device.is_null() {
            return;
        }
        eis_device_pause(device);
        eis_device_remove(device);
        eis_device_unref(device);
    }
}

#[cfg(has_libeis)]
unsafe fn propagate_device(device: *mut EisDevice) {
    unsafe {
        eis_device_add(device);
        eis_device_resume(device);
    }
}

#[cfg(has_libeis)]
unsafe fn teardown_client(client: &mut ClientState) {
    unsafe {
        if let Some(addr) = client.rel_pointer.take() {
            remove_device(addr as *mut EisDevice);
        }
        if let Some(addr) = client.keyboard.take() {
            remove_device(addr as *mut EisDevice);
        }
        if let Some(addr) = client.abs_pointer.take() {
            remove_device(addr as *mut EisDevice);
        }
        if client.seat != 0 {
            eis_seat_unref(client.seat as *mut EisSeat);
            client.seat = 0;
        }
    }
}

#[cfg(has_libeis)]
fn refresh_absolute_devices(hub: &mut EisHub) {
    let Some(client) = hub.client.as_mut() else {
        return;
    };
    unsafe {
        if let Some(addr) = client.abs_pointer.take() {
            remove_device(addr as *mut EisDevice);
        }
        if hub.viewports.is_empty() {
            return;
        }
        let seat = client.seat as *mut EisSeat;
        if seat.is_null() {
            return;
        }
        let device = eis_seat_new_device(seat);
        if device.is_null() {
            tracing::warn!("EIS: failed to create absolute pointer device");
            return;
        }
        eis_device_configure_name(device, c"metis absolute pointer".as_ptr());
        eis_device_configure_capability(device, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
        eis_device_configure_capability(device, EIS_DEVICE_CAP_BUTTON);
        eis_device_configure_capability(device, EIS_DEVICE_CAP_SCROLL);

        for viewport in hub.viewports.values() {
            let region = eis_device_new_region(device);
            if region.is_null() {
                continue;
            }
            if viewport.x != 0 || viewport.y != 0 {
                eis_region_set_offset(region, viewport.x, viewport.y);
            }
            eis_region_set_size(region, viewport.width, viewport.height);
            if viewport.scale > 0.0 {
                eis_region_set_physical_scale(region, viewport.scale);
            }
            let mapping = std::ffi::CString::new(viewport.mapping_id.as_str())
                .unwrap_or_else(|_| std::ffi::CString::new("metis:0").expect("static"));
            eis_region_set_mapping_id(region, mapping.as_ptr());
            eis_region_add(region);
            eis_region_unref(region);
        }

        propagate_device(device);
        client.abs_pointer = Some(device as usize);
        tracing::info!(
            regions = hub.viewports.len(),
            "EIS: absolute pointer device added"
        );
    }
}

#[cfg(not(has_libeis))]
fn refresh_absolute_devices(_hub: &mut EisHub) {}

#[cfg(has_libeis)]
unsafe fn attach_default_keymap(device: *mut EisDevice) -> bool {
    unsafe {
        use xkbcommon::xkb::{Context, KEYMAP_FORMAT_TEXT_V1, Keymap};

        let ctx = Context::new(0);
        let rules = std::env::var("XKB_DEFAULT_RULES").unwrap_or_else(|_| "evdev".into());
        let model = std::env::var("XKB_DEFAULT_MODEL").unwrap_or_default();
        let layout = std::env::var("XKB_DEFAULT_LAYOUT").unwrap_or_else(|_| "us".into());
        let variant = std::env::var("XKB_DEFAULT_VARIANT").unwrap_or_default();
        let keymap = match Keymap::new_from_names(&ctx, &rules, &model, &layout, &variant, None, 0)
        {
            Some(k) => k,
            None => {
                tracing::warn!("EIS: failed to compile XKB keymap");
                return false;
            }
        };
        let text = keymap.get_as_string(KEYMAP_FORMAT_TEXT_V1);
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            tracing::warn!("EIS: empty XKB keymap");
            return false;
        }

        let fd = libc::memfd_create(
            c"metis-xkb".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        );
        if fd < 0 {
            tracing::warn!("EIS: memfd_create failed for XKB keymap");
            return false;
        }
        let written = libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
        if written < 0 || written as usize != bytes.len() {
            let _ = libc::close(fd);
            tracing::warn!("EIS: failed to write XKB keymap to memfd");
            return false;
        }
        if libc::lseek(fd, 0, libc::SEEK_SET) < 0 {
            let _ = libc::close(fd);
            tracing::warn!("EIS: failed to seek XKB keymap memfd");
            return false;
        }

        let keymap_obj = eis_device_new_keymap(device, EIS_KEYMAP_TYPE_XKB, fd, bytes.len());
        let _ = libc::close(fd);
        if keymap_obj.is_null() {
            tracing::warn!("EIS: eis_device_new_keymap failed");
            return false;
        }
        eis_keymap_add(keymap_obj);
        eis_keymap_unref(keymap_obj);
        tracing::info!(bytes = bytes.len(), "EIS: attached XKB keymap");
        true
    }
}

#[cfg(has_libeis)]
unsafe fn add_relative_pointer(seat: *mut EisSeat, client: &mut ClientState) {
    unsafe {
        if client.rel_pointer.is_some() {
            return;
        }
        let device = eis_seat_new_device(seat);
        if device.is_null() {
            tracing::warn!("EIS: failed to create relative pointer device");
            return;
        }
        eis_device_configure_name(device, c"metis relative pointer".as_ptr());
        eis_device_configure_capability(device, EIS_DEVICE_CAP_POINTER);
        eis_device_configure_capability(device, EIS_DEVICE_CAP_BUTTON);
        eis_device_configure_capability(device, EIS_DEVICE_CAP_SCROLL);
        propagate_device(device);
        client.rel_pointer = Some(device as usize);
        tracing::info!("EIS: relative pointer device added");
    }
}

#[cfg(has_libeis)]
unsafe fn add_keyboard(seat: *mut EisSeat, client: &mut ClientState) {
    unsafe {
        if client.keyboard.is_some() {
            return;
        }
        let device = eis_seat_new_device(seat);
        if device.is_null() {
            tracing::warn!("EIS: failed to create keyboard device");
            return;
        }
        eis_device_configure_name(device, c"metis keyboard".as_ptr());
        eis_device_configure_capability(device, EIS_DEVICE_CAP_KEYBOARD);
        if !attach_default_keymap(device) {
            eis_device_unref(device);
            tracing::warn!("EIS: skipping keyboard — keymap unavailable");
            return;
        }
        propagate_device(device);
        client.keyboard = Some(device as usize);
        tracing::info!("EIS: keyboard device added");
    }
}

#[cfg(has_libeis)]
fn handle_seat_bind(ev: *mut EisEvent) {
    let wants_pointer = unsafe { eis_event_seat_has_capability(ev, EIS_DEVICE_CAP_POINTER) };
    let wants_keyboard = unsafe { eis_event_seat_has_capability(ev, EIS_DEVICE_CAP_KEYBOARD) };
    let wants_abs = unsafe { eis_event_seat_has_capability(ev, EIS_DEVICE_CAP_POINTER_ABSOLUTE) };

    let hub_arc = hub();
    let Ok(mut hub) = hub_arc.lock() else {
        return;
    };
    let seat = match hub.client.as_ref() {
        Some(c) if c.seat != 0 => c.seat as *mut EisSeat,
        _ => {
            tracing::warn!("EIS: seat bind before client connect");
            return;
        }
    };

    unsafe {
        let Some(client) = hub.client.as_mut() else {
            return;
        };

        if wants_pointer && client.rel_pointer.is_none() {
            add_relative_pointer(seat, client);
        } else if !wants_pointer && let Some(addr) = client.rel_pointer.take() {
            remove_device(addr as *mut EisDevice);
        }

        if wants_keyboard && client.keyboard.is_none() {
            add_keyboard(seat, client);
        } else if !wants_keyboard && let Some(addr) = client.keyboard.take() {
            remove_device(addr as *mut EisDevice);
        }
    }

    if wants_abs {
        refresh_absolute_devices(&mut hub);
    } else if let Some(client) = hub.client.as_mut()
        && let Some(addr) = client.abs_pointer.take()
    {
        unsafe {
            remove_device(addr as *mut EisDevice);
        }
    }

    tracing::info!(wants_pointer, wants_keyboard, wants_abs, "EIS seat bound");
}

#[cfg(has_libeis)]
fn desktop_coords_for_absolute(device: *mut EisDevice, x: f64, y: f64) -> Option<(f64, f64)> {
    if device.is_null() {
        return Some((x, y));
    }
    let region = unsafe { eis_device_get_region_at(device, x, y) };
    if region.is_null() {
        return Some((x, y));
    }
    let ox = unsafe { eis_region_get_x(region) };
    let oy = unsafe { eis_region_get_y(region) };
    Some((x + f64::from(ox), y + f64::from(oy)))
}

#[cfg(has_libeis)]
fn handle_client_connect(eis_client: *mut EisClient) {
    if !unsafe { eis_client_is_sender(eis_client) } {
        tracing::warn!("EIS: rejecting non-sender client");
        return;
    }

    unsafe { eis_client_connect(eis_client) };

    let seat = unsafe { eis_client_new_seat(eis_client, c"Metis seat".as_ptr()) };
    if seat.is_null() {
        tracing::error!("eis_client_new_seat failed");
        return;
    }

    unsafe {
        eis_seat_configure_capability(seat, EIS_DEVICE_CAP_KEYBOARD);
        eis_seat_configure_capability(seat, EIS_DEVICE_CAP_POINTER);
        eis_seat_configure_capability(seat, EIS_DEVICE_CAP_POINTER_ABSOLUTE);
        eis_seat_configure_capability(seat, EIS_DEVICE_CAP_BUTTON);
        eis_seat_configure_capability(seat, EIS_DEVICE_CAP_SCROLL);
        eis_seat_add(seat);
    }

    let seat_ref = unsafe { eis_seat_ref(seat) };
    if seat_ref.is_null() {
        tracing::error!("eis_seat_ref failed");
        unsafe {
            eis_seat_unref(seat);
        }
        return;
    }

    let hub_arc = hub();
    match hub_arc.lock() {
        Ok(mut hub) => {
            if let Some(mut old) = hub.client.take() {
                unsafe {
                    teardown_client(&mut old);
                }
            }
            hub.client = Some(ClientState {
                seat: seat_ref as usize,
                rel_pointer: None,
                keyboard: None,
                abs_pointer: None,
            });
        }
        _ => unsafe {
            eis_seat_unref(seat_ref);
        },
    }
    tracing::info!("EIS client connected");
}

#[cfg(has_libeis)]
fn handle_event(ev: *mut EisEvent) {
    let ty = unsafe { eis_event_get_type(ev) };
    match ty {
        EIS_EVENT_CLIENT_CONNECT => {
            let client = unsafe { eis_event_get_client(ev) };
            if client.is_null() {
                return;
            }
            handle_client_connect(client);
        }
        EIS_EVENT_SEAT_BIND => handle_seat_bind(ev),
        EIS_EVENT_POINTER_MOTION => {
            let dx = unsafe { eis_event_pointer_get_dx(ev) };
            let dy = unsafe { eis_event_pointer_get_dy(ev) };
            compositor_remote_input::inject_pointer_relative(dx, dy);
        }
        EIS_EVENT_POINTER_MOTION_ABSOLUTE => {
            let device = unsafe { eis_event_get_device(ev) };
            let x = unsafe { eis_event_pointer_get_absolute_x(ev) };
            let y = unsafe { eis_event_pointer_get_absolute_y(ev) };
            if let Some((dx, dy)) = desktop_coords_for_absolute(device, x, y) {
                compositor_remote_input::inject_pointer_absolute(dx, dy);
            }
        }
        EIS_EVENT_BUTTON_BUTTON => {
            let button = unsafe { eis_event_button_get_button(ev) };
            let pressed = unsafe { eis_event_button_get_is_press(ev) };
            compositor_remote_input::inject_pointer_button(button, pressed);
        }
        EIS_EVENT_SCROLL_DELTA => {
            let dx = unsafe { eis_event_scroll_get_dx(ev) };
            let dy = unsafe { eis_event_scroll_get_dy(ev) };
            compositor_remote_input::inject_pointer_scroll(dx, dy);
        }
        EIS_EVENT_KEYBOARD_KEY => {
            let key = unsafe { eis_event_keyboard_get_key(ev) };
            let pressed = unsafe { eis_event_keyboard_get_key_is_press(ev) };
            compositor_remote_input::inject_key(key, pressed);
        }
        EIS_EVENT_DEVICE_CLOSED => {
            let device = unsafe { eis_event_get_device(ev) };
            if device.is_null() {
                return;
            }
            let hub_arc = hub();
            if let Ok(mut hub) = hub_arc.lock()
                && let Some(client) = hub.client.as_mut()
            {
                let addr = device as usize;
                if client.rel_pointer == Some(addr) {
                    client.rel_pointer = None;
                }
                if client.keyboard == Some(addr) {
                    client.keyboard = None;
                }
                if client.abs_pointer == Some(addr) {
                    client.abs_pointer = None;
                }
            }
            unsafe {
                eis_device_unref(device);
            }
        }
        EIS_EVENT_CLIENT_DISCONNECT => {
            let hub_arc = hub();
            if let Ok(mut hub) = hub_arc.lock()
                && let Some(mut client) = hub.client.take()
            {
                unsafe {
                    teardown_client(&mut client);
                }
            }
            tracing::info!("EIS client disconnected");
        }
        other => {
            tracing::trace!(event_type = other, "EIS: ignored event");
        }
    }
}

#[cfg(not(has_libeis))]
fn handle_event(_ev: *mut EisEvent) {}
