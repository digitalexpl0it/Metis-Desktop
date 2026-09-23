use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::selection::data_device::{
    current_data_device_selection_userdata, request_data_device_client_selection,
    set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::set_primary_selection;

use crate::events::EventBus;
use crate::state::MetisState;

const TEXT_PREVIEW_CHARS: usize = 200;
const MAX_CLIPBOARD_BYTES: usize = 10 * 1024 * 1024;
const DRAIN_CLIPBOARD_BYTES_PER_TICK: usize = 512 * 1024;

const TEXT_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "TEXT",
    "STRING",
];
const IMAGE_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/bmp",
    "image/webp",
    "image/x-png",
];

/// Clipboard read in flight after a deferred capture request.
pub(crate) struct PendingClipboardRead {
    pub read: UnixStream,
    pub mime: String,
    pub data: Vec<u8>,
}

/// User data attached to compositor-owned clipboard offers (shell recall).
#[derive(Clone, Default)]
pub struct MetisSelectionUserData {
    /// In-memory payload (text recall and small data).
    pub offer: Option<CompositorClipboardOffer>,
    /// Image recall: read from disk only when a client requests paste.
    pub lazy_file: Option<(String, String)>,
}

#[derive(Clone)]
pub struct CompositorClipboardOffer {
    pub mime: String,
    pub data: Vec<u8>,
}

impl MetisSelectionUserData {
    /// Whether this offer can satisfy a paste without reading the file yet.
    /// Prefer this on the compositor thread — loading a lazy image sync-reads
    /// the full PNG and can stall the session if done inline.
    pub fn has_payload(&self) -> bool {
        if let Some(offer) = self.offer.as_ref() {
            return !offer.data.is_empty() && offer.data.len() <= MAX_CLIPBOARD_BYTES;
        }
        if let Some((path, _)) = self.lazy_file.as_ref() {
            return std::path::Path::new(path).is_file();
        }
        false
    }
}

fn normalize_mime(mime: &str) -> String {
    mime.split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase()
}

fn push_unique_mime(out: &mut Vec<String>, mime: &str) {
    let normalized = normalize_mime(mime);
    if normalized.is_empty() {
        return;
    }
    if out.iter().any(|m| normalize_mime(m) == normalized) {
        return;
    }
    out.push(normalized);
}

/// Whether a paste request can be satisfied by an offer mime.
pub fn selection_mime_satisfies(offer_mime: &str, request_mime: &str) -> bool {
    let offer = normalize_mime(offer_mime);
    let request = normalize_mime(request_mime);
    if offer == request {
        return true;
    }
    if offer.starts_with("image/") && request.starts_with("image/") {
        // Clients often negotiate a different image/* alias than we captured.
        return true;
    }
    matches!(
        (offer.as_str(), request.as_str()),
        ("image/png", "image/x-png")
            | ("image/x-png", "image/png")
            | ("image/jpeg", "image/jpg")
            | ("image/jpg", "image/jpeg")
    )
}

fn recall_mime_types(stored_mime: &str, path: Option<&str>) -> Vec<String> {
    let mut mimes = Vec::new();
    push_unique_mime(&mut mimes, stored_mime);
    if let Some(path) = path
        && let Some(ext) = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
    {
        match ext.to_ascii_lowercase().as_str() {
            "png" => {
                push_unique_mime(&mut mimes, "image/png");
                push_unique_mime(&mut mimes, "image/x-png");
            }
            "jpg" | "jpeg" => {
                push_unique_mime(&mut mimes, "image/jpeg");
                push_unique_mime(&mut mimes, "image/jpg");
            }
            "webp" => push_unique_mime(&mut mimes, "image/webp"),
            "bmp" => push_unique_mime(&mut mimes, "image/bmp"),
            _ => {}
        }
    }
    mimes
}

pub fn preferred_clipboard_mime(mimes: &[String]) -> Option<String> {
    for pref in TEXT_MIMES {
        if let Some(m) = mimes.iter().find(|m| m.as_str() == *pref) {
            return Some(m.clone());
        }
    }
    if mimes.iter().any(|m| m.starts_with("text/plain")) {
        return mimes.iter().find(|m| m.starts_with("text/plain")).cloned();
    }
    for pref in IMAGE_MIMES {
        if let Some(m) = mimes.iter().find(|m| normalize_mime(m) == *pref) {
            return Some(m.clone());
        }
    }
    mimes
        .iter()
        .find(|m| normalize_mime(m).starts_with("image/"))
        .cloned()
}

/// Write compositor-owned selection bytes to the fd offered by the client.
///
/// Always runs off the compositor thread. A blocking `write_all` on the event
/// loop deadlocks when the pasting client (especially XWayland/Electron) waits
/// for the compositor before draining the pipe — classic screenshot→paste TTY
/// lockup for region PNGs under a few hundred KB.
pub fn write_selection_to_fd(fd: std::os::unix::io::OwnedFd, data: Vec<u8>) {
    std::thread::spawn(move || {
        let mut file = std::fs::File::from(fd);
        if let Err(err) = file.write_all(&data) {
            tracing::debug!(?err, "failed to write compositor clipboard offer");
        }
    });
}

/// Serve a compositor-owned selection if possible.
/// Returns `Ok(())` when handled; returns the fd back when not applicable.
///
/// Lazy image offers are read and written entirely on a worker thread so paste
/// never blocks the compositor event loop.
pub fn serve_compositor_selection(
    user_data: &MetisSelectionUserData,
    request_mime: &str,
    fd: std::os::unix::io::OwnedFd,
) -> Result<(), std::os::unix::io::OwnedFd> {
    if let Some(offer) = user_data.offer.as_ref() {
        if !selection_mime_satisfies(&offer.mime, request_mime) {
            return Err(fd);
        }
        if offer.data.is_empty() || offer.data.len() > MAX_CLIPBOARD_BYTES {
            return Err(fd);
        }
        write_selection_to_fd(fd, offer.data.clone());
        return Ok(());
    }

    if let Some((path, mime)) = user_data.lazy_file.as_ref() {
        if !selection_mime_satisfies(mime, request_mime) {
            return Err(fd);
        }
        let path = path.clone();
        std::thread::spawn(move || {
            let data = match std::fs::read(&path) {
                Ok(data) if !data.is_empty() && data.len() <= MAX_CLIPBOARD_BYTES => data,
                Ok(_) => {
                    tracing::debug!(%path, "lazy clipboard file empty or over size cap");
                    return;
                }
                Err(err) => {
                    tracing::debug!(?err, %path, "lazy clipboard file read failed");
                    return;
                }
            };
            let mut file = std::fs::File::from(fd);
            if let Err(err) = file.write_all(&data) {
                tracing::debug!(?err, "failed to write compositor clipboard offer");
            }
        });
        return Ok(());
    }

    Err(fd)
}

impl MetisState {
    /// Queue capture for the next Wayland dispatch tick. `new_selection` fires
    /// before smithay commits the offer to the seat, so an immediate read fails.
    pub fn queue_clipboard_capture(&mut self, mimes: Vec<String>) {
        if self.clipboard_capture_suppressed > 0 {
            return;
        }
        if preferred_clipboard_mime(&mimes).is_some() {
            self.pending_clipboard_mimes = Some(mimes);
        }
    }

    /// Start a read once the seat selection is committed, then poll reads without
    /// blocking the compositor thread.
    pub fn flush_pending_clipboard_capture(&mut self) {
        if let Some(mimes) = self.pending_clipboard_mimes.take() {
            self.start_clipboard_capture(mimes);
        }
        self.drain_clipboard_reads();
    }

    fn start_clipboard_capture(&mut self, mimes: Vec<String>) {
        if self.clipboard_capture_suppressed > 0 {
            return;
        }
        if current_data_device_selection_userdata(&self.seat).is_some() {
            tracing::debug!("skip clipboard capture: compositor owns selection");
            return;
        }
        let Some(mime) = preferred_clipboard_mime(&mimes) else {
            tracing::debug!(?mimes, "clipboard capture: no supported mime");
            return;
        };

        let (read, write) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(?err, "clipboard pipe failed");
                return;
            }
        };

        if let Err(err) =
            request_data_device_client_selection(&self.seat, mime.clone(), write.into())
        {
            tracing::debug!(?err, ?mime, "clipboard read request failed");
            return;
        }

        let _ = read.set_nonblocking(true);
        self.pending_clipboard_reads.push(PendingClipboardRead {
            read,
            mime,
            data: Vec::new(),
        });
    }

    pub fn drain_clipboard_reads(&mut self) {
        use std::io::ErrorKind;

        let mut budget = DRAIN_CLIPBOARD_BYTES_PER_TICK;
        self.pending_clipboard_reads.retain_mut(|pending| {
            let mut chunk = [0u8; 65_536];
            loop {
                if budget == 0 {
                    return true;
                }
                match pending.read.read(&mut chunk) {
                    Ok(0) => {
                        if !pending.data.is_empty() {
                            emit_clipboard_changed(
                                &self.event_bus,
                                &pending.mime,
                                std::mem::take(&mut pending.data),
                            );
                        }
                        return false;
                    }
                    Ok(n) => {
                        if pending.data.len() + n > MAX_CLIPBOARD_BYTES {
                            tracing::debug!("clipboard payload exceeds size cap");
                            return false;
                        }
                        budget = budget.saturating_sub(n);
                        pending.data.extend_from_slice(&chunk[..n]);
                        if budget == 0 {
                            return true;
                        }
                    }
                    Err(err) if err.kind() == ErrorKind::WouldBlock => return true,
                    Err(err) => {
                        tracing::debug!(?err, "clipboard payload read failed");
                        return false;
                    }
                }
            }
        });
    }

    pub fn set_clipboard_from_command(
        &mut self,
        mime: String,
        text: Option<String>,
        image_path: Option<String>,
    ) -> Result<(), String> {
        let (user_data, mime_types, history) = if let Some(t) = text {
            let data = t.into_bytes();
            if data.is_empty() {
                return Err("clipboard data is empty".into());
            }
            if data.len() > MAX_CLIPBOARD_BYTES {
                return Err("clipboard payload exceeds size cap".into());
            }
            let offer = CompositorClipboardOffer {
                mime: mime.clone(),
                data: data.clone(),
            };
            (
                MetisSelectionUserData {
                    offer: Some(offer),
                    lazy_file: None,
                },
                recall_mime_types(&mime, None),
                ClipboardHistory::Bytes(data),
            )
        } else if let Some(path) = image_path {
            if !std::path::Path::new(&path).is_file() {
                return Err(format!("image not found: {path}"));
            }
            let len = std::fs::metadata(&path)
                .map_err(|err| format!("stat image: {err}"))?
                .len() as usize;
            if len == 0 {
                return Err("clipboard image is empty".into());
            }
            if len > MAX_CLIPBOARD_BYTES {
                return Err("clipboard payload exceeds size cap".into());
            }
            // Do not sync-read the PNG on the compositor thread (stalls the
            // session). Paste reads lazily off-thread; history reuses the path.
            (
                MetisSelectionUserData {
                    offer: None,
                    lazy_file: Some((path.clone(), mime.clone())),
                },
                recall_mime_types(&mime, Some(&path)),
                ClipboardHistory::ImagePath(path),
            )
        } else {
            return Err("SetClipboard requires text or image_path".into());
        };

        self.install_compositor_selection(mime_types, user_data);
        match history {
            ClipboardHistory::Bytes(data) => {
                emit_clipboard_changed(&self.event_bus, &mime, data);
            }
            ClipboardHistory::ImagePath(path) => {
                emit_clipboard_changed_image_path(&self.event_bus, &mime, path);
            }
        }
        Ok(())
    }

    fn install_compositor_selection(
        &mut self,
        mime_types: Vec<String>,
        user_data: MetisSelectionUserData,
    ) {
        self.clipboard_capture_suppressed += 1;
        set_data_device_selection(
            &self.display_handle,
            &self.seat,
            mime_types.clone(),
            user_data.clone(),
        );
        set_primary_selection(
            &self.display_handle,
            &self.seat,
            mime_types.clone(),
            user_data,
        );
        self.clipboard_capture_suppressed -= 1;

        // Electron / XWayland apps (e.g. Cursor) read the X11 clipboard, not wl_data_device.
        if let Some(xwm) = self.xwm.as_mut() {
            let mirror = Some(mime_types.clone());
            for target in [SelectionTarget::Clipboard, SelectionTarget::Primary] {
                if let Err(err) = xwm.new_selection(target, mirror.clone()) {
                    tracing::debug!(?err, ?target, "mirror compositor clipboard to XWayland");
                }
            }
        }
    }
}

enum ClipboardHistory {
    Bytes(Vec<u8>),
    ImagePath(String),
}

fn emit_clipboard_changed(bus: &EventBus, mime: &str, data: Vec<u8>) {
    if data.len() > MAX_CLIPBOARD_BYTES {
        return;
    }

    if mime.starts_with("text/") || matches!(mime, "UTF8_STRING" | "TEXT" | "STRING") {
        let text = String::from_utf8_lossy(&data);
        let preview = if text.chars().count() > TEXT_PREVIEW_CHARS {
            text.chars().take(TEXT_PREVIEW_CHARS).collect::<String>() + "…"
        } else {
            text.into_owned()
        };
        bus.emit(&metis_protocol::CompositorEvent::ClipboardChanged {
            mime: mime.to_string(),
            preview_text: Some(preview.clone()),
            image_path: None,
        });
        tracing::info!(
            mime,
            bytes = data.len(),
            has_text = true,
            has_image = false,
            "clipboard history captured"
        );
        return;
    }

    if !(mime.starts_with("image/") || normalize_mime(mime).starts_with("image/")) {
        return;
    }

    // Never write multi-MB PNGs on the compositor thread — that stalls the
    // session right as clients (and clipboard history) start negotiating paste.
    let bus = bus.clone();
    let mime = mime.to_string();
    std::thread::spawn(move || {
        let Some(path) = write_history_image(&mime, &data) else {
            return;
        };
        bus.emit(&metis_protocol::CompositorEvent::ClipboardChanged {
            mime: mime.clone(),
            preview_text: None,
            image_path: Some(path),
        });
        tracing::info!(
            mime = %mime,
            bytes = data.len(),
            has_text = false,
            has_image = true,
            "clipboard history captured"
        );
    });
}

/// History for a screenshot / image already on disk.
///
/// Copies into the clipboard cache on a worker thread so SetClipboard never
/// blocks the compositor, and history keeps a durable path if the capture file
/// is later moved/deleted.
fn emit_clipboard_changed_image_path(bus: &EventBus, mime: &str, image_path: String) {
    let bus = bus.clone();
    let mime = mime.to_string();
    std::thread::spawn(move || {
        let history_path = durable_history_image(&mime, &image_path).unwrap_or(image_path);
        bus.emit(&metis_protocol::CompositorEvent::ClipboardChanged {
            mime: mime.clone(),
            preview_text: None,
            image_path: Some(history_path),
        });
        tracing::info!(
            mime = %mime,
            has_text = false,
            has_image = true,
            "clipboard history captured"
        );
    });
}

fn history_image_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

fn write_history_image(mime: &str, data: &[u8]) -> Option<String> {
    let dir = clipboard_image_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "{}.{}",
        history_image_id(),
        image_file_extension(mime)
    ));
    std::fs::write(&path, data).ok()?;
    Some(path.to_string_lossy().into_owned())
}

fn durable_history_image(mime: &str, source: &str) -> Option<String> {
    let dir = clipboard_image_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let ext = std::path::Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp"))
        .unwrap_or_else(|| image_file_extension(mime).to_string());
    let dest = dir.join(format!("{}.{}", history_image_id(), ext));
    if source == dest.to_string_lossy() {
        return Some(source.to_string());
    }
    std::fs::copy(source, &dest).ok()?;
    Some(dest.to_string_lossy().into_owned())
}

fn image_file_extension(mime: &str) -> &'static str {
    let mime = normalize_mime(mime);
    if mime.contains("png") {
        "png"
    } else if mime.contains("jpeg") || mime.contains("jpg") {
        "jpg"
    } else if mime.contains("webp") {
        "webp"
    } else {
        "bmp"
    }
}

fn clipboard_image_dir() -> std::path::PathBuf {
    let dir = metis_protocol::runtime_dir().join("clipboard");
    if let Ok(runtime) = metis_protocol::ensure_runtime_dir() {
        let _ = std::fs::create_dir_all(runtime.join("clipboard"));
        let _ = metis_protocol::set_mode(&runtime.join("clipboard"), 0o700);
    }
    dir
}
