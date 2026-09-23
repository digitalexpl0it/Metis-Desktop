//! Removable volumes via Gio `VolumeMonitor` (USB, SD, optical/ISO, LUKS).
//!
//! No SMB/sFTP — network mounts are filtered out. Automount is left to
//! udisks2/gvfs; this module surfaces devices and Mount / Unmount / Eject /
//! Unlock actions for the edge-bar widget.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gio::prelude::*;
use glib::object::Cast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    Usb,
    Removable,
    Optical,
    Locked,
}

#[derive(Debug, Clone)]
pub struct VolumeEntry {
    pub id: String,
    pub label: String,
    pub kind: VolumeKind,
    pub mount_path: Option<PathBuf>,
    pub can_eject: bool,
    pub can_unmount: bool,
    /// Needs Mount (or Unlock for encrypted) before Open.
    pub needs_mount: bool,
    pub is_encrypted_locked: bool,
    pub tooltip: String,
}

thread_local! {
    static ENTRIES: RefCell<Vec<VolumeEntry>> = const { RefCell::new(Vec::new()) };
    static REFRESH: RefCell<Vec<Rc<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
    static MONITOR: RefCell<Option<gio::VolumeMonitor>> = const { RefCell::new(None) };
    static STARTED: RefCell<bool> = const { RefCell::new(false) };
}

pub fn snapshot() -> Vec<VolumeEntry> {
    ensure_started();
    ENTRIES.with(|c| c.borrow().clone())
}

pub fn register_refresh(cb: Rc<dyn Fn()>) {
    ensure_started();
    REFRESH.with(|cell| cell.borrow_mut().push(cb));
}

fn notify_refresh() {
    REFRESH.with(|cell| {
        for cb in cell.borrow().iter() {
            cb();
        }
    });
}

fn ensure_started() {
    STARTED.with(|started| {
        if *started.borrow() {
            return;
        }
        *started.borrow_mut() = true;
        let monitor = gio::VolumeMonitor::get();
        let rebuild = Rc::new(|| {
            rebuild_entries();
            notify_refresh();
        });
        {
            let rebuild = rebuild.clone();
            monitor.connect_volume_added(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_volume_removed(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_volume_changed(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_mount_added(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_mount_removed(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_mount_changed(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_drive_connected(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_drive_disconnected(move |_, _| rebuild());
        }
        {
            let rebuild = rebuild.clone();
            monitor.connect_drive_changed(move |_, _| rebuild());
        }
        MONITOR.with(|cell| *cell.borrow_mut() = Some(monitor));
        rebuild();
    });
}

fn rebuild_entries() {
    let monitor = gio::VolumeMonitor::get();
    let mut out: Vec<VolumeEntry> = Vec::new();
    let mut seen_paths: Vec<PathBuf> = Vec::new();

    for volume in monitor.volumes() {
        if !volume_is_interesting(&volume) {
            continue;
        }
        if let Some(entry) = entry_from_volume(&volume) {
            if let Some(path) = entry.mount_path.as_ref() {
                seen_paths.push(path.clone());
            }
            out.push(entry);
        }
    }

    // Some ISO/loop mounts appear as Mount without a Volume.
    for mount in monitor.mounts() {
        if mount_is_shadowed(&mount) {
            continue;
        }
        let Some(path) = mount_path(&mount) else {
            continue;
        };
        if seen_paths.iter().any(|p| p == &path) {
            continue;
        }
        if !path_looks_user_media(&path) {
            continue;
        }
        if is_system_mount_path(&path) {
            continue;
        }
        out.push(entry_from_mount(&mount, path));
    }

    out.sort_by_key(|a| a.label.to_lowercase());
    ENTRIES.with(|c| *c.borrow_mut() = out);
}

fn volume_is_interesting(volume: &gio::Volume) -> bool {
    if volume_is_network(volume) {
        return false;
    }
    if let Some(mount) = volume.get_mount()
        && let Some(path) = mount_path(&mount)
    {
        if is_system_mount_path(&path) {
            return false;
        }
        if path_looks_user_media(&path) {
            return true;
        }
    }
    if let Some(drive) = volume.drive() {
        if drive.is_removable() || drive.can_eject() {
            return true;
        }
        // Optical media often reports non-removable but can_eject / has media.
        if drive.has_media() && (drive.can_eject() || drive_looks_optical(&drive)) {
            return true;
        }
    }
    // Unmounted automountable volumes on removable media (incl. LUKS).
    volume.should_automount()
        && volume
            .drive()
            .is_some_and(|d| d.is_removable() || d.can_eject())
}

fn volume_is_network(volume: &gio::Volume) -> bool {
    for kind in ["class", "nfs", "smb", "sftp", "ftp", "http", "dav"] {
        if let Some(id) = volume.identifier(kind) {
            let lower = id.to_ascii_lowercase();
            if lower.contains("network")
                || lower.contains("smb")
                || lower.contains("nfs")
                || lower.contains("sftp")
                || lower.contains("ftp")
            {
                return true;
            }
        }
    }
    if let Some(mount) = volume.get_mount()
        && let Some(path) = mount_path(&mount)
    {
        let s = path.to_string_lossy();
        if s.starts_with("/run/user/") && s.contains("/gvfs/") {
            return true;
        }
    }
    false
}

fn drive_looks_optical(drive: &gio::Drive) -> bool {
    let name = drive.name().to_ascii_lowercase();
    name.contains("cd")
        || name.contains("dvd")
        || name.contains("bluray")
        || name.contains("optical")
        || name.contains("disc")
}

fn entry_from_volume(volume: &gio::Volume) -> Option<VolumeEntry> {
    let id = volume_id(volume);
    let mount = volume.get_mount();
    let mount_path = mount.as_ref().and_then(mount_path);
    if let Some(path) = mount_path.as_ref()
        && is_system_mount_path(path)
    {
        return None;
    }
    let label = friendly_volume_label(volume, mount.as_ref(), mount_path.as_deref());
    let encrypted = looks_encrypted(volume);
    let needs_mount = mount.is_none();
    let is_encrypted_locked = needs_mount && encrypted;
    let kind = if is_encrypted_locked {
        VolumeKind::Locked
    } else if volume.drive().as_ref().is_some_and(drive_looks_optical)
        || mount_path.as_ref().is_some_and(|p| path_looks_optical(p))
    {
        VolumeKind::Optical
    } else if volume.drive().is_some_and(|d| d.is_removable()) {
        VolumeKind::Usb
    } else {
        VolumeKind::Removable
    };
    let can_eject = volume.can_eject()
        || mount.as_ref().is_some_and(|m| m.can_eject())
        || volume.drive().is_some_and(|d| d.can_eject());
    let can_unmount = mount.as_ref().is_some_and(|m| m.can_unmount());
    let tooltip = format_tooltip(&label, mount_path.as_deref(), is_encrypted_locked);
    Some(VolumeEntry {
        id,
        label,
        kind,
        mount_path,
        can_eject,
        can_unmount,
        needs_mount,
        is_encrypted_locked,
        tooltip,
    })
}

fn entry_from_mount(mount: &gio::Mount, path: PathBuf) -> VolumeEntry {
    let label = friendly_mount_label(mount, &path);
    let id = format!("mount:{}", path.display());
    let kind = if path_looks_optical(&path) {
        VolumeKind::Optical
    } else {
        VolumeKind::Removable
    };
    let tooltip = format_tooltip(&label, Some(&path), false);
    VolumeEntry {
        id,
        label,
        kind,
        mount_path: Some(path),
        can_eject: mount.can_eject(),
        can_unmount: mount.can_unmount(),
        needs_mount: false,
        is_encrypted_locked: false,
        tooltip,
    }
}

/// User-facing media name. GIO's `volume.name()` is often a UUID / FAT serial
/// when the filesystem has no label — prefer the FS label, then the mount-folder
/// basename (what Nautilus shows), then a non-serial drive/volume name.
fn friendly_volume_label(
    volume: &gio::Volume,
    mount: Option<&gio::Mount>,
    mount_path: Option<&Path>,
) -> String {
    if let Some(label) = volume.identifier("label") {
        let label = label.trim();
        if !label.is_empty() && !looks_like_serial_or_uuid(label) {
            return label.to_string();
        }
    }
    if let Some(path) = mount_path
        && let Some(name) = media_basename(path)
    {
        return name;
    }
    if let Some(mount) = mount {
        let name = mount.name();
        let name = name.trim();
        if !name.is_empty() && !looks_like_serial_or_uuid(name) {
            return name.to_string();
        }
    }
    let vol_name = volume.name();
    let vol_name = vol_name.trim();
    if !vol_name.is_empty() && !looks_like_serial_or_uuid(vol_name) {
        return vol_name.to_string();
    }
    if let Some(drive) = volume.drive() {
        let name = drive.name();
        let name = name.trim();
        if !name.is_empty() && !looks_like_serial_or_uuid(name) && !drive_name_is_generic(name) {
            return name.to_string();
        }
    }
    if let Some(dev) = volume.identifier("unix-device")
        && let Some(base) = Path::new(dev.trim()).file_name()
    {
        let s = base.to_string_lossy();
        if !s.is_empty() {
            return format!("Drive ({s})");
        }
    }
    "Removable drive".into()
}

fn friendly_mount_label(mount: &gio::Mount, path: &Path) -> String {
    if let Some(name) = media_basename(path) {
        return name;
    }
    let name = mount.name();
    let name = name.trim();
    if !name.is_empty() && !looks_like_serial_or_uuid(name) {
        return name.to_string();
    }
    "Removable drive".into()
}

/// Last path component under `/media` / `/run/media` / `/mnt`, skipping UUID-like names.
fn media_basename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.trim();
    if name.is_empty() || looks_like_serial_or_uuid(name) {
        return None;
    }
    Some(name.to_string())
}

fn drive_name_is_generic(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "usb drive"
            | "usb disk"
            | "usb device"
            | "removable disk"
            | "removable media"
            | "sd card"
            | "card reader"
            | "floppy disk"
            | "cd-rom"
            | "cdrom"
            | "dvd"
            | "dvd-rom"
    )
}

/// FAT volume ids (`XXXX-XXXX`), GPT/MBR UUIDs, and long hex serials that GIO
/// often surfaces as the volume "name" when no filesystem label is set.
fn looks_like_serial_or_uuid(name: &str) -> bool {
    let s = name.trim();
    if s.is_empty() {
        return false;
    }
    // FAT16/32 volume serial: ABCD-1234
    if s.len() == 9 {
        let b = s.as_bytes();
        if b[4] == b'-'
            && s[..4].chars().all(|c| c.is_ascii_hexdigit())
            && s[5..].chars().all(|c| c.is_ascii_hexdigit())
        {
            return true;
        }
    }
    // Canonical UUID: 8-4-4-4-12 hex
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 5
        && parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return true;
    }
    // Long unbroken hex / alphanumeric serials (no spaces), common for raw device ids.
    let alnum: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if alnum.len() >= 12 && alnum.chars().all(|c| c.is_ascii_hexdigit()) && !s.contains(' ') {
        return true;
    }
    false
}

fn volume_id(volume: &gio::Volume) -> String {
    if let Some(uuid) = volume.identifier("uuid") {
        return format!("uuid:{uuid}");
    }
    if let Some(unix) = volume.identifier("unix-device") {
        return format!("dev:{unix}");
    }
    format!("name:{}", volume.name())
}

fn looks_encrypted(volume: &gio::Volume) -> bool {
    let name = volume.name().to_ascii_lowercase();
    if name.contains("encrypt") || name.contains("luks") || name.contains("locked") {
        return true;
    }
    for kind in ["type", "filesystem", "fs.type", "class"] {
        if let Some(id) = volume.identifier(kind) {
            let lower = id.to_ascii_lowercase();
            if lower.contains("crypto") || lower.contains("luks") {
                return true;
            }
        }
    }
    false
}

fn mount_path(mount: &gio::Mount) -> Option<PathBuf> {
    mount.root().path()
}

fn mount_is_shadowed(mount: &gio::Mount) -> bool {
    mount.volume().is_some()
}

fn path_looks_user_media(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/media/") || s.starts_with("/run/media/") || s.starts_with("/mnt/")
}

fn path_looks_optical(path: &Path) -> bool {
    let s = path.to_string_lossy().to_ascii_lowercase();
    s.contains("cdrom") || s.contains("dvd") || s.contains("optical")
}

fn is_system_mount_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    matches!(
        s.as_ref(),
        "/" | "/boot"
            | "/boot/efi"
            | "/home"
            | "/usr"
            | "/var"
            | "/tmp"
            | "/opt"
            | "/srv"
            | "/root"
    ) || s.starts_with("/boot/")
        || s.starts_with("/snap/")
        || s.starts_with("/var/lib/snapd/")
        || s.starts_with("/var/lib/flatpak/")
        || s.starts_with("/nix/")
        || s.starts_with("/ostree/")
        || s.starts_with("/System/")
}

fn format_tooltip(label: &str, path: Option<&Path>, locked: bool) -> String {
    if locked {
        return format!("{label} (locked — click to unlock)");
    }
    match path {
        Some(p) => format!("{label}\n{}", p.display()),
        None => format!("{label} (not mounted)"),
    }
}

fn find_volume(id: &str) -> Option<gio::Volume> {
    let monitor = gio::VolumeMonitor::get();
    monitor.volumes().into_iter().find(|v| volume_id(v) == id)
}

fn find_mount(id: &str) -> Option<gio::Mount> {
    let monitor = gio::VolumeMonitor::get();
    if let Some(path) = id.strip_prefix("mount:") {
        let want = PathBuf::from(path);
        return monitor
            .mounts()
            .into_iter()
            .find(|m| mount_path(m).as_ref() == Some(&want));
    }
    find_volume(id)?.get_mount()
}

fn mount_operation() -> gio::MountOperation {
    // Parent window None — GTK still shows the passphrase dialog via the
    // GtkMountOperation default when available; fall back to Gio base.
    gtk::MountOperation::new(None::<&gtk::Window>).upcast()
}

/// Left-click: open if mounted, else mount/unlock then open.
pub fn activate(id: &str) {
    ensure_started();
    if let Some(path) = snapshot()
        .into_iter()
        .find(|e| e.id == id)
        .and_then(|e| e.mount_path)
    {
        open_in_file_manager(&path);
        return;
    }
    let id = id.to_string();
    mount_then_open(&id);
}

fn mount_then_open(id: &str) {
    let Some(volume) = find_volume(id) else {
        tracing::warn!(%id, "volume not found for mount");
        return;
    };
    let id_owned = id.to_string();
    let op = mount_operation();
    volume.mount(
        gio::MountMountFlags::empty(),
        Some(&op),
        gio::Cancellable::NONE,
        move |result| {
            if let Err(err) = result {
                tracing::warn!(%err, "volume mount/unlock failed");
                toast_error(&format!("Could not mount volume: {err}"));
                return;
            }
            rebuild_entries();
            notify_refresh();
            if let Some(path) = snapshot()
                .into_iter()
                .find(|e| e.id == id_owned)
                .and_then(|e| e.mount_path)
            {
                open_in_file_manager(&path);
            }
        },
    );
}

pub fn mount_volume(id: &str) {
    ensure_started();
    mount_then_open(id);
}

pub fn unmount(id: &str) {
    ensure_started();
    let Some(mount) = find_mount(id) else {
        tracing::warn!(%id, "mount not found for unmount");
        return;
    };
    let op = mount_operation();
    mount.unmount_with_operation(
        gio::MountUnmountFlags::empty(),
        Some(&op),
        gio::Cancellable::NONE,
        move |result| {
            if let Err(err) = result {
                tracing::warn!(%err, "unmount failed");
                toast_error(&format!(
                    "Could not unmount — close apps using this drive.\n{err}"
                ));
                return;
            }
            rebuild_entries();
            notify_refresh();
        },
    );
}

pub fn eject(id: &str) {
    ensure_started();
    let op = mount_operation();
    if let Some(mount) = find_mount(id)
        && mount.can_eject()
    {
        mount.eject_with_operation(
            gio::MountUnmountFlags::empty(),
            Some(&op),
            gio::Cancellable::NONE,
            move |result| {
                if let Err(err) = result {
                    tracing::warn!(%err, "eject failed");
                    toast_error(&format!(
                        "Could not eject — close apps using this drive.\n{err}"
                    ));
                    return;
                }
                rebuild_entries();
                notify_refresh();
            },
        );
        return;
    }
    if let Some(volume) = find_volume(id) {
        if volume.can_eject() {
            volume.eject_with_operation(
                gio::MountUnmountFlags::empty(),
                Some(&op),
                gio::Cancellable::NONE,
                move |result| {
                    if let Err(err) = result {
                        tracing::warn!(%err, "eject failed");
                        toast_error(&format!("Could not eject drive.\n{err}"));
                        return;
                    }
                    rebuild_entries();
                    notify_refresh();
                },
            );
            return;
        }
        if let Some(drive) = volume.drive()
            && drive.can_eject()
        {
            drive.eject_with_operation(
                gio::MountUnmountFlags::empty(),
                Some(&op),
                gio::Cancellable::NONE,
                move |result| {
                    if let Err(err) = result {
                        tracing::warn!(%err, "drive eject failed");
                        toast_error(&format!("Could not eject drive.\n{err}"));
                        return;
                    }
                    rebuild_entries();
                    notify_refresh();
                },
            );
            return;
        }
    }
    // Fall back to unmount when eject is unavailable.
    unmount(id);
}

pub fn open_in_file_manager(path: &Path) {
    let path_s = path.to_string_lossy().into_owned();
    let argv = if let Some(fm) = metis_config::resolve_file_manager() {
        vec![fm, path_s]
    } else {
        vec!["xdg-open".into(), path_s]
    };
    if let Err(err) = crate::compositor::launch_argv(argv) {
        tracing::warn!(%err, path = %path.display(), "failed to open volume in file manager");
        toast_error(&format!("Could not open {}", path.display()));
    }
}

fn toast_error(message: &str) {
    crate::ui::toast::show(&crate::services::BarNotification::internal(
        crate::services::NotificationKind::Error,
        "Removable drive",
        message,
    ));
}

pub fn icon_name(kind: VolumeKind) -> &'static str {
    match kind {
        VolumeKind::Usb => "drive-harddisk-usb-symbolic",
        VolumeKind::Removable => "media-removable-symbolic",
        VolumeKind::Optical => "media-optical-symbolic",
        VolumeKind::Locked => "drive-harddisk-encrypted-symbolic",
    }
}
