//! Client spawn helpers and GPU/cursor env for processes launched by the
//! compositor. Sibling `impl MetisState` extracted from `state.rs` (mechanical
//! split; no behavior change).

use std::time::Duration;

use crate::state::{MetisState, PendingGamingLaunch};

/// Cursor theme/size for nested clients. Never calls D-Bus — a synchronous
/// `gsettings` in `spawn_client` blocked the compositor event loop during shell
/// startup (especially after `--import-env`), which GNOME reported as
/// "Unknown is not responding".
pub(crate) fn resolve_client_cursor_env() -> (String, String) {
    fn cursor_icon_dirs() -> Vec<std::path::PathBuf> {
        let mut dirs = Vec::new();
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(std::path::PathBuf::from(format!("{home}/.icons")));
            dirs.push(std::path::PathBuf::from(format!(
                "{home}/.local/share/icons"
            )));
        }
        dirs.push(std::path::PathBuf::from("/usr/share/icons"));
        dirs.push(std::path::PathBuf::from("/usr/local/share/icons"));
        dirs
    }
    fn resolve_cursor_theme(name: &str) -> Option<String> {
        fn inner(name: &str, dirs: &[std::path::PathBuf], depth: u8) -> Option<String> {
            if name.is_empty() || depth > 8 {
                return None;
            }
            if dirs.iter().any(|d| d.join(name).join("cursors").is_dir()) {
                return Some(name.to_string());
            }
            for d in dirs {
                let Ok(text) = std::fs::read_to_string(d.join(name).join("index.theme")) else {
                    continue;
                };
                for line in text.lines() {
                    if let Some(rest) = line.trim().strip_prefix("Inherits") {
                        let rest = rest.trim_start_matches([' ', '=']).trim();
                        for parent in rest.split(',') {
                            if let Some(found) = inner(parent.trim(), dirs, depth + 1) {
                                return Some(found);
                            }
                        }
                    }
                }
            }
            None
        }
        inner(name, &cursor_icon_dirs(), 0)
    }
    fn gtk_settings_value(home: &str, key: &str) -> Option<String> {
        for rel in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
            let path = std::path::Path::new(home).join(".config").join(rel);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('#') || !line.contains('=') {
                    continue;
                }
                let (k, v) = line.split_once('=')?;
                if k.trim() == key {
                    let v = v.trim().trim_matches('"');
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
        None
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let theme_pref = std::env::var("XCURSOR_THEME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| gtk_settings_value(&home, "gtk-cursor-theme-name"))
        .unwrap_or_else(|| "default".into());
    let cursor_theme = resolve_cursor_theme(&theme_pref)
        .or_else(|| resolve_cursor_theme("default"))
        .or_else(|| resolve_cursor_theme("Yaru"))
        .or_else(|| resolve_cursor_theme("Adwaita"))
        .unwrap_or_else(|| "Adwaita".into());
    let cursor_size = std::env::var("XCURSOR_SIZE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| gtk_settings_value(&home, "gtk-cursor-theme-size"))
        .unwrap_or_else(|| "24".into());
    (cursor_theme, cursor_size)
}

/// GPU steering hint for spawned clients, derived from the render node the
/// compositor actually renders on (DRM/udev backend only). Exported so that
/// clients which do not negotiate a device over Wayland dmabuf feedback
/// (XWayland, Proton/Vulkan, native GL apps) default to the *same* GPU the
/// compositor uses — avoiding the "game picks the wrong card / black screen"
/// class of bugs on hybrid systems.
#[derive(Clone, Debug)]
pub(crate) struct ClientGpuHint {
    /// Mesa GL device selector, e.g. `pci-0000_03_00_0` (`DRI_PRIME`).
    dri_prime: Option<String>,
    /// Mesa Vulkan default-device selector, e.g. `1002:73df`
    /// (`MESA_VK_DEVICE_SELECT`).
    vk_select: Option<String>,
    /// Compositor render node (`/dev/dri/renderD*`) for Chromium/Electron
    /// `--render-node-override`. Newer Electron (Cursor) inherits this from
    /// dmabuf feedback; Electron 32 / GitHub Desktop does not and opens the
    /// wrong GBM device → SCANOUT `ENOMEM` GPU crash-loops.
    render_node_path: Option<std::path::PathBuf>,
}

impl ClientGpuHint {
    /// Resolve the PCI identity + render-node path of a DRM render node.
    /// Returns `None` only when neither PCI identity nor a device path can be
    /// resolved (virtual/vgem with no `/dev/dri` path).
    pub(crate) fn from_render_node(node: &smithay::backend::drm::DrmNode) -> Option<Self> {
        let render_node_path = {
            let path = crate::rudp_host::render_node_device_path(node);
            path.exists().then_some(path)
        };
        let base = format!("/sys/dev/char/{}:{}/device", node.major(), node.minor());
        // The PCI address is the basename of the resolved `device` symlink,
        // e.g. `/sys/.../0000:03:00.0` -> `0000:03:00.0`.
        let dri_prime = std::fs::canonicalize(&base).ok().and_then(|p| {
            p.file_name()
                .map(|f| format!("pci-{}", f.to_string_lossy().replace([':', '.'], "_")))
        });
        let read_hex = |name: &str| -> Option<String> {
            std::fs::read_to_string(format!("{base}/{name}"))
                .ok()
                .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
                .filter(|s| !s.is_empty())
        };
        let vk_select = match (read_hex("vendor"), read_hex("device")) {
            (Some(v), Some(d)) => Some(format!("{v}:{d}")),
            _ => None,
        };
        if dri_prime.is_none() && vk_select.is_none() && render_node_path.is_none() {
            return None;
        }
        Some(Self {
            dri_prime,
            vk_select,
            render_node_path,
        })
    }

    /// `/dev/dri/renderD*` for Chromium `--render-node-override`, if known.
    pub(crate) fn render_node_path(&self) -> Option<&std::path::Path> {
        self.render_node_path.as_deref()
    }

    /// Apply the hint to a spawned command, only for keys the surrounding
    /// environment has not already set (so Steam launch options such as
    /// `DRI_PRIME=1`, `prime-run`, or NVIDIA offload vars still win per game).
    fn apply(&self, cmd: &mut std::process::Command) {
        if let Some(tag) = &self.dri_prime
            && std::env::var_os("DRI_PRIME").is_none()
        {
            cmd.env("DRI_PRIME", tag);
        }
        if let Some(sel) = &self.vk_select
            && std::env::var_os("MESA_VK_DEVICE_SELECT").is_none()
        {
            cmd.env("MESA_VK_DEVICE_SELECT", sel);
        }
    }
}

/// Append Chromium/Electron argv so the GPU process opens the compositor's
/// render node (same as Cursor's self-applied `--render-node-override`).
fn append_ozone_render_node_override(argv: &mut Vec<String>, render_node: &std::path::Path) {
    if argv
        .iter()
        .any(|a| a == "--render-node-override" || a.starts_with("--render-node-override="))
    {
        return;
    }
    argv.push(format!("--render-node-override={}", render_node.display()));
}

/// Force `--ozone-platform=x11` when the argv does not already pin a platform.
fn append_ozone_platform_x11(argv: &mut Vec<String>) {
    if argv
        .iter()
        .any(|a| a == "--ozone-platform" || a.starts_with("--ozone-platform="))
    {
        return;
    }
    argv.push("--ozone-platform=x11".into());
}

#[cfg(test)]
mod ozone_render_node_tests {
    use super::append_ozone_render_node_override;
    use std::path::Path;

    #[test]
    fn appends_render_node_override_once() {
        let mut argv = vec!["github-desktop".to_string()];
        append_ozone_render_node_override(&mut argv, Path::new("/dev/dri/renderD129"));
        assert_eq!(
            argv,
            vec![
                "github-desktop".to_string(),
                "--render-node-override=/dev/dri/renderD129".to_string(),
            ]
        );
        append_ozone_render_node_override(&mut argv, Path::new("/dev/dri/renderD128"));
        assert_eq!(argv.len(), 2, "must not duplicate the flag");
    }
}

/// PRIME render-offload steering for the discrete high-power GPU on a hybrid
/// (Optimus / muxless) laptop.
///
/// The compositor renders and scans out on the iGPU that owns the panel (see
/// `pick_primary_gpu`). `ClientGpuHint` then pins *every* spawned client to that
/// same iGPU — which is right for lightweight desktop apps but catastrophic for
/// games and Steam Big Picture: they get forced onto the weak integrated GPU
/// while the dGPU sits idle (the "Big Picture loads very slowly" report). This
/// steers game/launcher processes onto the dGPU instead; their buffers are
/// imported cross-GPU by the compositor for scanout (standard PRIME offload,
/// exactly what GNOME/KDE do for Wayland clients).
#[derive(Clone, Debug)]
pub(crate) enum DgpuOffload {
    /// Proprietary NVIDIA: offload via the NVIDIA GLX/Vulkan stack.
    Nvidia,
    /// A Mesa-driven dGPU (AMD / Intel Arc / Nouveau): select its render node.
    Mesa {
        dri_prime: String,
        vk_select: Option<String>,
    },
}

impl DgpuOffload {
    /// Detect a discrete GPU distinct from the compositor's display GPU by
    /// scanning `/sys/class/drm/card*`. Returns `None` on single-GPU systems.
    pub(crate) fn detect(display_node: &smithay::backend::drm::DrmNode) -> Option<Self> {
        let pci_addr_of_node = |node: &smithay::backend::drm::DrmNode| -> Option<String> {
            let base = format!("/sys/dev/char/{}:{}/device", node.major(), node.minor());
            std::fs::canonicalize(&base)
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        };
        let display_pci = pci_addr_of_node(display_node);

        for entry in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Only `cardN` nodes; skip connectors like `card2-eDP-1`.
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let dev = entry.path().join("device");
            // A real GPU exposes `boot_vga`; the discrete one has `boot_vga=0`.
            if !dev.join("boot_vga").exists() {
                continue;
            }
            let pci = std::fs::canonicalize(&dev)
                .ok()
                .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()));
            // Skip the display GPU itself — we only want the *other* card.
            if pci.is_none() || pci == display_pci {
                continue;
            }
            let read_hex = |field: &str| -> Option<String> {
                std::fs::read_to_string(dev.join(field))
                    .ok()
                    .map(|s| s.trim().trim_start_matches("0x").to_lowercase())
                    .filter(|s| !s.is_empty())
            };
            let vendor = read_hex("vendor");
            let device = read_hex("device");
            match vendor.as_deref() {
                // NVIDIA — but only if the proprietary stack is actually present;
                // with nouveau there is no `__NV_PRIME_RENDER_OFFLOAD`, so fall
                // through to the Mesa path instead.
                Some("10de") if std::path::Path::new("/proc/driver/nvidia").exists() => {
                    return Some(DgpuOffload::Nvidia);
                }
                Some(_) => {
                    let dri_prime = pci.map(|p| format!("pci-{}", p.replace([':', '.'], "_")))?;
                    let vk_select = match (vendor, device) {
                        (Some(v), Some(d)) => Some(format!("{v}:{d}")),
                        _ => None,
                    };
                    return Some(DgpuOffload::Mesa {
                        dri_prime,
                        vk_select,
                    });
                }
                None => continue,
            }
        }
        None
    }

    /// Apply the offload env to a spawned command, deferring to any value the
    /// surrounding environment (or a Steam per-game launch option) already set.
    fn apply(&self, cmd: &mut std::process::Command) {
        let set_if_unset = |cmd: &mut std::process::Command, key: &str, val: &str| {
            if std::env::var_os(key).is_none() {
                cmd.env(key, val);
            }
        };
        match self {
            DgpuOffload::Nvidia => {
                // Proprietary NVIDIA PRIME render offload: route GLX + Vulkan
                // (DXVK/VKD3D/Proton) to the NVIDIA GPU. Deliberately do NOT set
                // `DRI_PRIME`/`MESA_VK_DEVICE_SELECT` — those are Mesa-only and
                // would pin Vulkan back onto the iGPU, fighting the offload.
                set_if_unset(cmd, "__NV_PRIME_RENDER_OFFLOAD", "1");
                set_if_unset(cmd, "__GLX_VENDOR_LIBRARY_NAME", "nvidia");
                set_if_unset(cmd, "__VK_LAYER_NV_optimus", "NVIDIA_only");
            }
            DgpuOffload::Mesa {
                dri_prime,
                vk_select,
            } => {
                set_if_unset(cmd, "DRI_PRIME", dri_prime);
                if let Some(sel) = vk_select {
                    set_if_unset(cmd, "MESA_VK_DEVICE_SELECT", sel);
                }
            }
        }
    }
}

/// Environment shared by every client the compositor spawns (shell, settings,
/// menu launches). GTK hardening avoids portal/a11y stalls in a bare session.
#[allow(clippy::too_many_arguments)]
fn apply_spawned_client_env(
    cmd: &mut std::process::Command,
    program: &str,
    socket: &std::ffi::OsStr,
    xdisplay: Option<u32>,
    xdisplay_gaming: Option<u32>,
    client_gpu: Option<&ClientGpuHint>,
    dgpu_offload: Option<&DgpuOffload>,
    prefer_dgpu: bool,
    use_gaming_xwayland: bool,
) {
    cmd.env("WAYLAND_DISPLAY", socket);
    cmd.env("METIS_SESSION", "1");
    // Phase 18 D: gaming X11 class is independent of GPU offload / battery.
    let display = if use_gaming_xwayland {
        xdisplay_gaming.or(xdisplay)
    } else {
        xdisplay
    };
    match display {
        Some(n) => {
            tracing::debug!(
                program,
                display = n,
                gaming_x11 = use_gaming_xwayland,
                "spawn DISPLAY"
            );
            cmd.env("DISPLAY", format!(":{n}"));
        }
        None => {
            cmd.env_remove("DISPLAY");
        }
    }
    cmd.env("GDK_BACKEND", "wayland");
    let profile = metis_config::load_graphics_profile();
    let compat = metis_config::effective_graphics_compatibility(profile);
    cmd.env(
        "METIS_GRAPHICS_PROFILE",
        metis_config::effective_graphics_profile_label(profile),
    );
    // Drive GTK4 / libadwaita clients to Metis light/dark even when the Settings
    // portal is slow or unavailable (Nautilus, etc.).
    let theme_mode = metis_config::load_theme_preference().unwrap_or(metis_config::ThemeMode::Dark);
    match metis_config::appearance_gtk_theme_env(theme_mode) {
        Some(gtk_theme) => {
            cmd.env("GTK_THEME", gtk_theme);
        }
        None => {
            cmd.env_remove("GTK_THEME");
        }
    }
    // Shell always prefers Cairo for layer-shell stability unless overridden.
    // Compatibility mode also forces Cairo for every other GTK client (VMs).
    // Settings: GTK 4.22+ defaults to GskVulkanRenderer, which hitch-scrolls on
    // hybrid NVIDIA under Metis (pause then catch-up). Pin GL unless overridden;
    // do not inherit the session's shell Cairo `GSK_RENDERER`.
    if program.contains("metis-shell") {
        let renderer = std::env::var("METIS_SHELL_GSK_RENDERER")
            .or_else(|_| std::env::var("GSK_RENDERER"))
            .unwrap_or_else(|_| "cairo".into());
        cmd.env("GSK_RENDERER", renderer);
        // Never let the edge bar / desktop-widgets block ~25s inside gtk::init
        // waiting on org.freedesktop.portal.Settings. A leftover greeter or
        // GNOME portal on the session bus causes exactly the "wallpaper then
        // frozen shell" hang after login. Other apps still use the portal stack.
        let gdk_debug = std::env::var("GDK_DEBUG").unwrap_or_default();
        if gdk_debug.is_empty() {
            cmd.env("GDK_DEBUG", "no-portals");
        } else if !gdk_debug
            .split(',')
            .any(|p| p == "no-portals" || p == "portals")
        {
            cmd.env("GDK_DEBUG", format!("{gdk_debug},no-portals"));
        }
    } else if program.contains("metis-settings") {
        if compat {
            cmd.env("GSK_RENDERER", "cairo");
        } else if let Ok(renderer) = std::env::var("METIS_SETTINGS_GSK_RENDERER") {
            cmd.env("GSK_RENDERER", renderer);
        } else {
            // Cairo: GL/Vulkan hitch-scroll for seconds on tall Settings pages
            // under hybrid NVIDIA. Users can opt back into gl via the env var.
            cmd.env("GSK_RENDERER", "cairo");
        }
    } else if compat {
        cmd.env("GSK_RENDERER", "cairo");
    } else {
        cmd.env_remove("GSK_RENDERER");
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        cmd.env("XDG_RUNTIME_DIR", runtime);
    }
    cmd.env("GTK_A11Y", "none");
    cmd.env("NO_AT_BRIDGE", "1");
    // Prefer native Wayland for Electron/Chromium apps. Metis is a Wayland
    // compositor and its XWayland path is a fallback; Electron's XWayland
    // map/unmap lifecycle is unstable here (Claude Desktop juggles windows on
    // launch and cleanly quits via `window-all-closed` — it "opens then closes").
    // `ELECTRON_OZONE_PLATFORM_HINT=auto` is the standard opt-in most Electron
    // apps honor; Claude Desktop's launcher force-passes `--ozone-platform=x11`
    // and only switches to Wayland via `CLAUDE_USE_WAYLAND=1`. Both defer to a
    // value the surrounding session already set, so a user can force XWayland.
    //
    // Exception: GitHub Desktop (Electron 32) crash-loops its GPU process on
    // native Wayland under Metis (`RGBA_8888` SCANOUT → Intel ENOMEM). Prefer
    // XWayland for those apps — same path that is fast under GNOME.
    if std::env::var_os("ELECTRON_OZONE_PLATFORM_HINT").is_none() {
        if metis_config::command_prefers_x11_ozone(program) {
            cmd.env("ELECTRON_OZONE_PLATFORM_HINT", "x11");
        } else {
            cmd.env("ELECTRON_OZONE_PLATFORM_HINT", "auto");
        }
    }
    if std::env::var_os("CLAUDE_USE_WAYLAND").is_none() {
        cmd.env("CLAUDE_USE_WAYLAND", "1");
    }
    // Firefox: prefer native Wayland (WebGL / canvas compose on Metis, not XWayland).
    if std::env::var_os("MOZ_ENABLE_WAYLAND").is_none() {
        cmd.env("MOZ_ENABLE_WAYLAND", "1");
    }
    // Nested dev sessions run inside GNOME/KDE — disable GTK's portal proxy so
    // startup does not block on the host portal stack.
    if std::env::var_os("METIS_NESTED").is_some() {
        let gdk_debug = std::env::var("GDK_DEBUG").unwrap_or_default();
        if gdk_debug.is_empty() {
            cmd.env("GDK_DEBUG", "no-portals");
        } else if !gdk_debug
            .split(',')
            .any(|p| p == "no-portals" || p == "portals")
        {
            cmd.env("GDK_DEBUG", format!("{gdk_debug},no-portals"));
        } else {
            cmd.env("GDK_DEBUG", gdk_debug);
        }
    }
    // GPU steering (DRM backend only), unless the user opted out with
    // METIS_NO_CLIENT_GPU. Per-game overrides still win since every `apply` only
    // sets keys that are not already present.
    //   * Games / launchers (`prefer_dgpu`) are pushed onto the discrete GPU via
    //     PRIME render offload when one exists — otherwise the display-GPU hint
    //     is a harmless fallback (single-GPU systems).
    //   * Everything else stays on the compositor's (display) GPU, which is the
    //     power-efficient iGPU on a hybrid laptop.
    //   * Chromium / Electron / Ozone shells get *no* DRI_PRIME / NVIDIA offload
    //     vars: they pick the GPU from Wayland dmabuf feedback. Forcing a card
    //     (even the iGPU) races their GBM SCANOUT probe and can crash the GPU
    //     process (GitHub Desktop / Chrome multi-second hangs on hybrid Intel).
    if std::env::var_os("METIS_NO_CLIENT_GPU").is_none()
        && !metis_config::command_is_ozone_client(program)
    {
        match (prefer_dgpu, dgpu_offload) {
            (true, Some(dgpu)) => dgpu.apply(cmd),
            _ => {
                if let Some(hint) = client_gpu {
                    hint.apply(cmd);
                }
            }
        }
    }
}

impl MetisState {
    pub(crate) fn spawn_widgets_process(&mut self, program: &str) {
        let before = self.child_processes.len();
        let token = Self::new_widgets_ipc_token();
        self.widgets_ipc_token = Some(token.clone());
        let argv = metis_protocol::split_command_line(program);
        self.spawn_client_argv_with_env(
            &argv,
            &[
                ("METIS_IPC_TOKEN", token.as_str()),
                ("METIS_IPC_SCOPE", "widgets"),
            ],
        );
        self.widgets_last_spawn = Some(std::time::Instant::now());
        if self.child_processes.len() > before {
            if let Some(child) = self.child_processes.last() {
                self.widgets_pid = Some(child.id());
            }
        } else {
            self.widgets_pid = None;
            self.widgets_ipc_token = None;
        }
    }

    /// Collect exit status for finished children so they leave the process table.
    pub(crate) fn reap_exited_children(&mut self) {
        let mut i = 0;
        while i < self.child_processes.len() {
            match self.child_processes[i].try_wait() {
                Ok(None) => i += 1,
                Ok(Some(status)) => {
                    // `try_wait` already reaped; drop the handle (clippy wants `.wait()`
                    // but a second wait after a successful try_wait is redundant).
                    #[allow(clippy::zombie_processes)]
                    let child = self.child_processes.remove(i);
                    let pid = child.id();
                    if self.widgets_pid == Some(pid) {
                        self.widgets_pid = None;
                        self.widgets_ipc_token = None;
                    }
                    tracing::debug!(pid, ?status, "reaped exited client process");
                }
                Err(err) => {
                    #[allow(clippy::zombie_processes)]
                    let child = self.child_processes.remove(i);
                    let pid = child.id();
                    if self.widgets_pid == Some(pid) {
                        self.widgets_pid = None;
                        self.widgets_ipc_token = None;
                    }
                    tracing::debug!(pid, %err, "dropped client process handle");
                }
            }
        }
    }

    /// Rate-limited respawn if the widgets process exits (crash / OOM).
    pub(crate) fn maybe_respawn_widgets(&mut self) {
        let Some(cmd) = self.widgets_cmd.clone() else {
            return;
        };

        self.reap_exited_children();

        if self.widgets_pid.is_some() {
            return; // still running
        }

        if self.widgets_last_spawn.is_some() {
            tracing::warn!("desktop-widgets process exited — respawning");
        }

        let min_gap = Duration::from_secs(3);
        if self
            .widgets_last_spawn
            .is_some_and(|t| t.elapsed() < min_gap)
        {
            return;
        }
        self.spawn_widgets_process(&cmd);
    }

    pub fn spawn_client(&mut self, program: &str) {
        let argv = metis_protocol::split_command_line(program);
        self.spawn_client_argv(&argv);
    }

    fn new_widgets_ipc_token() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        format!("{nanos:032x}{pid:08x}")
    }

    /// Spawn a client from argv — never via `sh -c`.
    pub fn spawn_client_argv(&mut self, argv: &[String]) {
        self.spawn_client_argv_with_env(argv, &[]);
    }

    pub fn spawn_client_argv_with_env(&mut self, argv: &[String], extra_env: &[(&str, &str)]) {
        if argv.is_empty() {
            tracing::warn!("spawn_client_argv: empty argv");
            return;
        }
        // Metis binaries (shell, settings) live alongside the compositor in the
        // cargo target dir, which is usually not on PATH. Resolve a bare program
        // name to its sibling-of-current-exe absolute path so `Launch` works.
        let mut argv = argv.to_vec();
        if !argv[0].contains('/')
            && let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let candidate = dir.join(&argv[0]);
            if candidate.is_file() {
                argv[0] = candidate.display().to_string();
            }
        }

        // Metis-owned Steam tweaks (MangoHud / Gamescope) — never Steam VDF writes.
        let tweaks = metis_config::apply_steam_launch_tweaks(&argv, &self.gaming_config);
        let mut argv = tweaks.argv;
        let tweak_env = tweaks.env;
        let program_joined = argv.join(" ");
        // Older Electron (GitHub Desktop / Electron 32) does not inherit the
        // compositor render node from dmabuf feedback the way Cursor (Electron
        // 42) does. Pin `--render-node-override` so Wayland GPU processes open
        // the same `/dev/dri/renderD*` Metis scans out on.
        if metis_config::command_is_ozone_client(&program_joined)
            && let Some(path) = self.client_gpu.as_ref().and_then(|h| h.render_node_path())
        {
            append_ozone_render_node_override(&mut argv, path);
        }
        // GitHub Desktop: native Wayland GPU crash-loops on this stack — pin
        // XWayland (argv + ELECTRON_OZONE_PLATFORM_HINT in apply_spawned_client_env).
        if metis_config::command_prefers_x11_ozone(&program_joined) {
            append_ozone_platform_x11(&mut argv);
        }
        let program_joined = argv.join(" ");
        let app_cfg = metis_config::load_app_config();
        let use_gaming_xwayland = app_cfg.xwayland_mode == metis_config::XwaylandMode::Isolated
            && metis_config::command_uses_gaming_xwayland(
                &program_joined,
                &app_cfg.xwayland_policy,
            );

        if use_gaming_xwayland && self.xdisplay_gaming.is_none() {
            tracing::info!(
                program = %program_joined,
                "spawn: queueing until gaming XWayland is ready"
            );
            let mut queued_env: Vec<(String, String)> = extra_env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect();
            for (k, v) in tweak_env {
                queued_env.push((k, v));
            }
            self.pending_gaming_launches.push(PendingGamingLaunch {
                argv: argv.clone(),
                extra_env: queued_env,
            });
            self.ensure_gaming_xwayland();
            return;
        }

        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        for (k, v) in &tweak_env {
            cmd.env(k, v);
        }

        // Games/launchers run on the discrete GPU; desktop apps on the iGPU.
        // `gaming.json` + METIS_GAME_GPU override the heuristic.
        let prefer_dgpu =
            metis_config::prefer_dgpu_for_launch(&program_joined, &self.gaming_config);
        if prefer_dgpu && self.dgpu_offload.is_some() {
            tracing::info!(
                program = %program_joined,
                "spawn: steering launch onto discrete GPU (PRIME offload)"
            );
        }
        if use_gaming_xwayland {
            tracing::info!(
                program = %program_joined,
                display = ?self.xdisplay_gaming,
                "spawn: gaming XWayland class"
            );
        }
        apply_spawned_client_env(
            &mut cmd,
            &program_joined,
            &self.socket_name,
            self.xdisplay,
            self.xdisplay_gaming,
            self.client_gpu.as_ref(),
            self.dgpu_offload.as_ref(),
            prefer_dgpu,
            use_gaming_xwayland,
        );
        cmd.env("XCURSOR_THEME", &self.client_cursor_theme);
        cmd.env("XCURSOR_SIZE", &self.client_cursor_size);

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                tracing::info!(
                    program = %program_joined,
                    pid,
                    wayland_display = ?self.socket_name,
                    "spawned client"
                );
                if metis_config::command_prefers_dgpu(&program_joined) && prefer_dgpu {
                    self.event_bus
                        .emit(&metis_protocol::CompositorEvent::GameSession {
                            active: true,
                            label: Some(program_joined.clone()),
                            pid: Some(pid),
                        });
                }
                self.child_processes.push(child);
            }
            Err(err) => {
                tracing::warn!(program = %program_joined, %err, "failed to spawn client")
            }
        }
    }
}
