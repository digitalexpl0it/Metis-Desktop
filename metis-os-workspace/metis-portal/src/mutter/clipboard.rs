//! Remote-desktop clipboard bridge for gnome-remote-desktop.
//!
//! Mutter's `org.gnome.Mutter.RemoteDesktop.Session` uses option dicts and fd
//! passing. Local Metis clipboard may include text and images (durable paths
//! under `$XDG_RUNTIME_DIR/metis/clipboard/`); both are synced to RDP within a
//! 10 MiB cap matching the compositor.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use zbus::zvariant::Value;

use crate::compositor_ipc;

/// Match compositor `MAX_CLIPBOARD_BYTES`.
const MAX_CLIPBOARD_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct ClipboardSession {
    inner: Arc<Mutex<Inner>>,
    conn: zbus::Connection,
    session_path: String,
}

struct Inner {
    enabled: bool,
    /// True while the remote RDP client owns the clipboard (after SetSelection).
    remote_owner: bool,
    remote_mimes: Vec<String>,
    transfer_serial: u32,
    pending_write_serial: Option<u32>,
    pending_write_mime: Option<String>,
    last_local: Option<LocalClip>,
}

#[derive(Clone)]
struct LocalClip {
    mime: String,
    text: Option<String>,
    image_path: Option<String>,
}

impl ClipboardSession {
    pub fn new(conn: zbus::Connection, session_path: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                enabled: false,
                remote_owner: false,
                remote_mimes: Vec::new(),
                transfer_serial: 0,
                pending_write_serial: None,
                pending_write_mime: None,
                last_local: None,
            })),
            conn,
            session_path,
        }
    }

    pub fn enable(&self, options: &HashMap<&str, Value<'_>>) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "clipboard lock".to_string())?;
        if inner.enabled {
            return Err("Already enabled".into());
        }
        inner.enabled = true;
        inner.remote_owner = false;
        inner.remote_mimes = mime_types_from_options(options);
        drop(inner);
        if let Some(local) = self.inner.lock().ok().and_then(|i| i.last_local.clone()) {
            self.emit_owner_changed(false, &local_mimes(&local));
        }
        Ok(())
    }

    pub fn disable(&self) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "clipboard lock".to_string())?;
        if !inner.enabled {
            return Err("Was not enabled".into());
        }
        inner.enabled = false;
        inner.remote_owner = false;
        inner.remote_mimes.clear();
        inner.pending_write_serial = None;
        inner.pending_write_mime = None;
        Ok(())
    }

    pub fn set_selection(&self, options: &HashMap<&str, Value<'_>>) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "clipboard lock".to_string())?;
        if !inner.enabled {
            return Err("Clipboard not enabled".into());
        }
        let mimes = mime_types_from_options(options);
        if mimes.is_empty() {
            inner.remote_owner = false;
            inner.remote_mimes.clear();
            inner.pending_write_mime = None;
            drop(inner);
            self.emit_owner_changed(false, &[]);
        } else {
            inner.remote_owner = true;
            inner.remote_mimes = mimes.clone();
            drop(inner);
            self.emit_owner_changed(true, &mimes);
            // Eagerly pull remote content into Metis so paste works without a
            // separate local SelectionSource request path.
            if let Some(mime) = preferred_remote_mime(&mimes) {
                self.request_transfer(&mime);
            }
        }
        Ok(())
    }

    /// GRD writes remote clipboard bytes to the returned fd, then calls SelectionWriteDone.
    pub fn selection_write(&self, serial: u32) -> Result<OwnedFd, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "clipboard lock".to_string())?;
        if !inner.enabled {
            return Err("Clipboard not enabled".into());
        }
        if !inner.remote_owner {
            return Err("No current selection owned".into());
        }
        if inner.pending_write_serial.is_some() && inner.pending_write_serial != Some(serial) {
            tracing::warn!(
                serial,
                expected = ?inner.pending_write_serial,
                "SelectionWrite serial mismatch — accepting anyway"
            );
        }
        let mime = inner
            .pending_write_mime
            .clone()
            .or_else(|| preferred_remote_mime(&inner.remote_mimes))
            .unwrap_or_else(|| "text/plain;charset=utf-8".into());
        inner.pending_write_serial = None;
        inner.pending_write_mime = None;
        drop(inner);

        let (read_fd, write_fd) = pipe::pipe().map_err(|err| format!("pipe: {err}"))?;
        let read_for_thread = read_fd
            .try_clone()
            .map_err(|err| format!("pipe clone: {err}"))?;
        std::thread::Builder::new()
            .name("metis-rd-clip".into())
            .spawn(move || {
                let mut file = std::fs::File::from(read_for_thread);
                let mut buf = Vec::new();
                let mut chunk = [0u8; 64 * 1024];
                loop {
                    match file.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            if buf.len() + n > MAX_CLIPBOARD_BYTES {
                                tracing::warn!(
                                    %mime,
                                    "RDP clipboard write exceeded {MAX_CLIPBOARD_BYTES} bytes — dropping"
                                );
                                return;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                        }
                        Err(err) => {
                            tracing::warn!(%err, %mime, "RDP clipboard write read failed");
                            return;
                        }
                    }
                }
                if buf.is_empty() {
                    return;
                }
                apply_remote_clipboard_bytes(&mime, &buf);
            })
            .ok();
        Ok(write_fd)
    }

    pub fn selection_write_done(&self, _serial: u32, _success: bool) -> Result<(), String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "clipboard lock".to_string())?;
        if !inner.enabled {
            return Err("Clipboard not enabled".into());
        }
        Ok(())
    }

    /// GRD reads local clipboard bytes from the returned fd.
    pub fn selection_read(&self, mime_type: &str) -> Result<OwnedFd, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "clipboard lock".to_string())?;
        if !inner.enabled {
            return Err("Clipboard not enabled".into());
        }
        if inner.remote_owner {
            return Err("Tried to read own selection".into());
        }
        let Some(local) = inner.last_local.clone() else {
            return Err("No selection owner available".into());
        };
        drop(inner);

        let data = local_clip_bytes(&local, mime_type)?;
        let (read_fd, write_fd) = pipe::pipe().map_err(|err| format!("pipe: {err}"))?;
        let write_for_thread = write_fd
            .try_clone()
            .map_err(|err| format!("pipe clone: {err}"))?;
        std::thread::spawn(move || {
            let mut file = std::fs::File::from(write_for_thread);
            let _ = file.write_all(&data);
        });
        Ok(read_fd)
    }

    pub fn request_transfer(&self, mime_type: &str) {
        let serial = {
            let mut inner = match self.inner.lock() {
                Ok(i) => i,
                Err(_) => return,
            };
            if !inner.enabled || !inner.remote_owner {
                return;
            }
            inner.transfer_serial = inner.transfer_serial.saturating_add(1);
            inner.pending_write_serial = Some(inner.transfer_serial);
            inner.pending_write_mime = Some(mime_type.to_string());
            inner.transfer_serial
        };
        std::mem::drop(self.conn.emit_signal(
            None::<&str>,
            self.session_path.as_str(),
            "org.gnome.Mutter.RemoteDesktop.Session",
            "SelectionTransfer",
            &(mime_type, serial),
        ));
    }

    pub fn on_local_clipboard_changed(
        &self,
        mime: &str,
        preview_text: Option<&str>,
        image_path: Option<&str>,
    ) {
        let enabled = self.inner.lock().map(|i| i.enabled).unwrap_or(false);
        if !enabled {
            return;
        }

        let is_text = preview_text.is_some()
            || mime.contains("text")
            || mime.eq_ignore_ascii_case("UTF8_STRING");
        let is_image = image_path.is_some() || is_image_mime(mime);
        if !is_text && !is_image {
            return;
        }

        let local = LocalClip {
            mime: mime.to_string(),
            text: preview_text.map(str::to_string),
            image_path: image_path.filter(|p| !p.is_empty()).map(str::to_string),
        };
        let mimes = local_mimes(&local);
        if mimes.is_empty() {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_local = Some(local);
            inner.remote_owner = false;
        }
        self.emit_owner_changed(false, &mimes);
    }

    fn emit_owner_changed(&self, session_is_owner: bool, mime_types: &[String]) {
        let mut options: HashMap<String, Value<'_>> = HashMap::new();
        if !mime_types.is_empty() {
            options.insert("mime-types".into(), Value::from(mime_types.to_vec()));
            options.insert("session-is-owner".into(), Value::from(session_is_owner));
        }
        std::mem::drop(self.conn.emit_signal(
            None::<&str>,
            self.session_path.as_str(),
            "org.gnome.Mutter.RemoteDesktop.Session",
            "SelectionOwnerChanged",
            &(options,),
        ));
    }
}

fn mime_types_from_options(options: &HashMap<&str, Value<'_>>) -> Vec<String> {
    if let Some(raw) = options.get("mime-types")
        && let Some(list) = parse_string_array(raw)
    {
        let filtered: Vec<String> = list
            .into_iter()
            .filter(|m| is_text_mime(m) || is_image_mime(m))
            .collect();
        if !filtered.is_empty() {
            return filtered;
        }
    }
    // Fallback when GRD omits mime-types or sends an opaque variant.
    vec![
        "text/plain;charset=utf-8".into(),
        "text/plain".into(),
        "UTF8_STRING".into(),
        "image/png".into(),
        "image/jpeg".into(),
        "image/bmp".into(),
    ]
}

fn parse_string_array(value: &Value<'_>) -> Option<Vec<String>> {
    if let Ok(list) = <Vec<String>>::try_from(value.clone())
        && !list.is_empty()
    {
        return Some(list);
    }
    match value {
        Value::Array(arr) => {
            let mut out = Vec::new();
            for item in arr.iter() {
                if let Ok(s) = String::try_from(item.clone()) {
                    out.push(s);
                } else if let Ok(s) = <&str>::try_from(item) {
                    out.push(s.to_string());
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
        Value::Str(s) => Some(vec![s.as_str().to_string()]),
        _ => None,
    }
}

fn preferred_remote_mime(mimes: &[String]) -> Option<String> {
    mimes
        .iter()
        .find(|m| is_text_mime(m))
        .cloned()
        .or_else(|| mimes.iter().find(|m| is_image_mime(m)).cloned())
}

fn local_mimes(local: &LocalClip) -> Vec<String> {
    let mut out = Vec::new();
    if local.text.is_some()
        || local.mime.contains("text")
        || local.mime.eq_ignore_ascii_case("UTF8_STRING")
    {
        out.extend([
            "text/plain;charset=utf-8".into(),
            "text/plain".into(),
            "UTF8_STRING".into(),
        ]);
    }
    if let Some(path) = &local.image_path {
        let mime = image_mime_for_path(path).unwrap_or_else(|| normalize_image_mime(&local.mime));
        if is_image_mime(&mime) {
            // Advertise the concrete type first, then common GRD aliases.
            push_unique(&mut out, mime.clone());
            if mime != "image/png" {
                push_unique(&mut out, "image/png".into());
            }
            if mime != "image/bmp" {
                push_unique(&mut out, "image/bmp".into());
            }
            if mime != "image/jpeg" {
                push_unique(&mut out, "image/jpeg".into());
            }
        }
    } else if is_image_mime(&local.mime) {
        push_unique(&mut out, normalize_image_mime(&local.mime));
    }
    out
}

fn local_clip_bytes(local: &LocalClip, mime_type: &str) -> Result<Vec<u8>, String> {
    if is_text_mime(mime_type)
        && let Some(text) = &local.text
    {
        let bytes = text.as_bytes();
        if bytes.len() > MAX_CLIPBOARD_BYTES {
            return Err("clipboard text too large".into());
        }
        return Ok(bytes.to_vec());
    }
    if is_image_mime(mime_type) {
        let Some(path) = &local.image_path else {
            return Err("no local image clipboard".into());
        };
        let path = validate_clipboard_image_path(path)?;
        let meta = std::fs::metadata(&path).map_err(|e| format!("image stat: {e}"))?;
        if meta.len() as usize > MAX_CLIPBOARD_BYTES {
            return Err("clipboard image too large".into());
        }
        // Serve native file bytes. RDP clients often ask for image/bmp while we
        // store PNG — still return the file; GRD/clients that cannot decode will
        // fall through other advertised types on a later SelectionRead.
        let data = std::fs::read(&path).map_err(|e| format!("image read: {e}"))?;
        if data.len() > MAX_CLIPBOARD_BYTES {
            return Err("clipboard image too large".into());
        }
        return Ok(data);
    }
    Err(format!("unsupported clipboard mime {mime_type}"))
}

fn apply_remote_clipboard_bytes(mime: &str, buf: &[u8]) {
    if is_text_mime(mime) {
        match std::str::from_utf8(buf) {
            Ok(text) => {
                compositor_ipc::set_clipboard("text/plain;charset=utf-8", Some(text), None);
            }
            Err(_) => {
                tracing::warn!(%mime, "remote text clipboard was not UTF-8 — dropping");
            }
        }
        return;
    }
    if is_image_mime(mime) {
        match write_remote_image(mime, buf) {
            Ok(path) => {
                let store_mime = normalize_image_mime(mime);
                compositor_ipc::set_clipboard(&store_mime, None, Some(&path));
            }
            Err(err) => tracing::warn!(%err, %mime, "failed to store remote image clipboard"),
        }
        return;
    }
    tracing::debug!(%mime, "ignoring unsupported remote clipboard mime");
}

fn write_remote_image(mime: &str, data: &[u8]) -> Result<String, String> {
    if data.len() > MAX_CLIPBOARD_BYTES {
        return Err("image exceeds size cap".into());
    }
    let dir = clipboard_image_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir clipboard: {e}"))?;
    let ext = image_extension(mime);
    let name = format!(
        "rdp-{}-{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        ext
    );
    let path = dir.join(name);
    std::fs::write(&path, data).map_err(|e| format!("write image: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

fn clipboard_image_dir() -> Result<PathBuf, String> {
    Ok(metis_protocol::runtime_dir().join("clipboard"))
}

fn validate_clipboard_image_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    let canon = path
        .canonicalize()
        .map_err(|e| format!("invalid clipboard path: {e}"))?;
    let root = clipboard_image_dir()?
        .canonicalize()
        .unwrap_or_else(|_| metis_protocol::runtime_dir().join("clipboard"));
    if !canon.starts_with(&root) {
        return Err("clipboard image path outside runtime clipboard dir".into());
    }
    Ok(canon)
}

fn is_text_mime(mime: &str) -> bool {
    let m = mime.trim();
    m.contains("text") || m.eq_ignore_ascii_case("UTF8_STRING") || m.eq_ignore_ascii_case("TEXT")
}

fn is_image_mime(mime: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    m.starts_with("image/png")
        || m.starts_with("image/jpeg")
        || m.starts_with("image/jpg")
        || m.starts_with("image/bmp")
        || m.starts_with("image/x-ms-bmp")
        || m.starts_with("image/x-bmp")
        || m.contains("png") && m.starts_with("image/")
        || m.contains("jpeg") && m.starts_with("image/")
}

fn normalize_image_mime(mime: &str) -> String {
    let m = mime.to_ascii_lowercase();
    if m.contains("jpeg") || m.contains("jpg") {
        "image/jpeg".into()
    } else if m.contains("bmp") {
        "image/bmp".into()
    } else {
        "image/png".into()
    }
}

fn image_mime_for_path(path: &str) -> Option<String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" => Some("image/png".into()),
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "bmp" => Some("image/bmp".into()),
        "webp" => None, // GRD/RDP: skip webp advertise; native file still local-only
        _ => None,
    }
}

fn image_extension(mime: &str) -> &'static str {
    let m = mime.to_ascii_lowercase();
    if m.contains("jpeg") || m.contains("jpg") {
        "jpg"
    } else if m.contains("bmp") {
        "bmp"
    } else {
        "png"
    }
}

fn push_unique(out: &mut Vec<String>, mime: String) {
    if !out.iter().any(|m| m == &mime) {
        out.push(mime);
    }
}

mod pipe {
    use std::os::fd::{FromRawFd, OwnedFd};

    pub fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
        let mut fds = [0i32; 2];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: pipe2 returned valid fds.
        unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn temp_xdg_runtime() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "metis-portal-clip-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp xdg runtime");
        dir
    }

    #[test]
    fn local_mimes_advertise_image_and_text() {
        let local = LocalClip {
            mime: "image/png".into(),
            text: Some("hi".into()),
            image_path: Some("/tmp/x.png".into()),
        };
        let mimes = local_mimes(&local);
        assert!(mimes.iter().any(|m| m.contains("text")));
        assert!(mimes.iter().any(|m| m == "image/png"));
    }

    #[test]
    fn preferred_mime_prefers_text_then_image() {
        let mimes = vec!["image/png".into(), "text/plain".into()];
        assert_eq!(preferred_remote_mime(&mimes).as_deref(), Some("text/plain"));
        let images = vec!["image/jpeg".into(), "image/png".into()];
        assert_eq!(
            preferred_remote_mime(&images).as_deref(),
            Some("image/jpeg")
        );
    }

    #[test]
    fn mime_allowlist_filters_non_text_non_image() {
        assert!(is_text_mime("text/plain"));
        assert!(is_text_mime("UTF8_STRING"));
        assert!(is_image_mime("image/png"));
        assert!(is_image_mime("image/jpeg"));
        assert!(!is_text_mime("application/octet-stream"));
        assert!(!is_image_mime("application/octet-stream"));
        assert!(!is_image_mime("text/plain"));

        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert(
            "mime-types",
            Value::from(vec![
                "application/octet-stream".to_string(),
                "text/plain".to_string(),
                "image/png".to_string(),
            ]),
        );
        let filtered = mime_types_from_options(&options);
        assert_eq!(
            filtered,
            vec!["text/plain".to_string(), "image/png".to_string()]
        );
    }

    #[test]
    fn local_clip_bytes_enforces_text_size_cap() {
        let oversized = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        let local = LocalClip {
            mime: "text/plain".into(),
            text: Some(oversized),
            image_path: None,
        };
        let err = local_clip_bytes(&local, "text/plain").unwrap_err();
        assert!(err.contains("too large"));

        let ok = LocalClip {
            mime: "text/plain".into(),
            text: Some("hi".into()),
            image_path: None,
        };
        assert_eq!(local_clip_bytes(&ok, "text/plain").unwrap(), b"hi");
    }

    #[test]
    fn clipboard_image_path_allowlist_and_size_cap() {
        let _guard = env_lock();
        let xdg = temp_xdg_runtime();
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &xdg) };

        let clip_dir = clipboard_image_dir().expect("clipboard dir");
        std::fs::create_dir_all(&clip_dir).expect("mkdir clipboard");
        let allowed = clip_dir.join("ok.png");
        std::fs::write(&allowed, b"png-bytes").expect("write allowed");

        let path = validate_clipboard_image_path(allowed.to_str().unwrap()).expect("allowlisted");
        assert_eq!(path, allowed.canonicalize().unwrap());

        let outside = xdg.join("escape.png");
        std::fs::write(&outside, b"nope").expect("write outside");
        let denied = validate_clipboard_image_path(outside.to_str().unwrap()).unwrap_err();
        assert!(denied.contains("outside runtime clipboard dir"));

        let huge = clip_dir.join("huge.png");
        {
            let f = std::fs::File::create(&huge).expect("create huge");
            f.set_len((MAX_CLIPBOARD_BYTES as u64) + 1)
                .expect("set_len");
        }
        let local = LocalClip {
            mime: "image/png".into(),
            text: None,
            image_path: Some(huge.to_string_lossy().into_owned()),
        };
        let err = local_clip_bytes(&local, "image/png").unwrap_err();
        assert!(err.contains("too large"));

        assert!(write_remote_image("image/png", &vec![0u8; MAX_CLIPBOARD_BYTES + 1]).is_err());

        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        let _ = std::fs::remove_dir_all(&xdg);
    }
}
