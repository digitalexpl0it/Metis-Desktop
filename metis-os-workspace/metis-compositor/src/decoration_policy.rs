//! Heuristics for when Metis draws server-side window chrome (SSD) vs leaving
//! decorations to the client (CSD).
//!
//! **Default: Metis decorates.** Strip chrome only when we are confident the
//! client draws its own (known `app_id`, or `xdg-decoration` ClientSide).
//!
//! While a likely-GTK window is still starting up (`app_id` empty but
//! `xdg-decoration` already bound), treat it as CSD so Metis does not paint a
//! second titlebar on headerbar apps (Cheese, Chromium, …). Terminals that bind
//! decoration and request `ServerSide` still get Metis SSD.

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;

/// No native window chrome — Metis draws titlebar + traffic lights.
///
/// Terminals are listed by BOTH their bare Wayland `app_id` (what they actually
/// report at runtime — `kitty`, `Alacritty`, `foot`, …) and any reverse-DNS
/// desktop id, because `id_matches_list` only accepts an exact or `.suffix`
/// match. Listing only the reverse-DNS form silently let terminals fall through
/// to the CSD fallback and lose Metis chrome.
const SSD_APP_IDS: &[&str] = &[
    // Alacritty reports `Alacritty` (WM class); keep the reverse-DNS form too.
    "alacritty",
    "org.alacritty",
    // Ghostty.
    "ghostty",
    "com.mitchellh.ghostty",
    // foot / footclient.
    "foot",
    "footclient",
    "org.codeberg.dnkl.foot",
    // WezTerm.
    "wezterm",
    "org.wezfurlong.wezterm",
    // kitty reports bare `kitty`.
    "kitty",
    "org.kitty",
    "net.kovidgoyal.kitty",
    // Other common terminals with no native titlebar.
    "xterm",
    "st",
    "urxvt",
    "com.metis.Settings",
    // Metis' own helper toplevels drop their GTK titlebar under a Metis session
    // and rely on this entry for chrome.
    "com.metis.Screenshot",
    "com.metis.Viewer",
    // GitHub Desktop (Electron): Flatpak + native package app_ids. The blanket
    // `io.github.*` CSD rule would strip Metis chrome, but the app draws no
    // Wayland titlebar — same class of bug as frameless `chromium` Electron shells.
    "io.github.shiftey.Desktop",
    "io.github.shiftkey.GitHubDesktop",
    "com.github.GitHubDesktop",
    "github desktop",
    // Generic Electron shells (Claude Desktop, …) often report bare `electron`
    // and bind xdg-decoration before drawing any chrome.
    "electron",
    // Claude Desktop StartupWMClass / desktop basename (when not reporting `electron`).
    "claude",
    "claude-desktop",
    // Media players with no native Wayland chrome.
    "mpv",
];

/// Built-in headerbar — never draw Metis SSD on top (non-GNOME / non-prefix
/// entries only; GNOME/XFCE/LibreOffice/… are covered by [`id_looks_csd`] rules).
const CSD_APP_IDS: &[&str] = &[
    "dev.zed.Zed",
    "com.obsproject.Studio",
    "com.slack.Slack",
    "com.discordapp.Discord",
    "com.spotify.Client",
    "com.visualstudio.code",
    "code-oss",
    "Cursor",
    "cursor",
    "com.cursor.Cursor",
    "dev.cursor.Cursor",
    "com.github.PintaProject.Pinta",
    "org.mozilla.firefox",
    "org.mozilla.Firefox",
    "firefox_firefox",
    "org.gnome.Cheese",
    "cheese",
    // Ubuntu App Center / snap-store (Flutter + GTK variants).
    "io.snapcraft.Store",
    "snap-store",
    "snap_store",
    "ubuntu-software",
    // GNOME Help — bare id when desktop file is not reverse-DNS.
    "yelp",
    // Mousepad / Thunar (GTK3 CSD or client chrome). Bare WM-class ids are what
    // XWayland and many XFCE builds report at runtime.
    "mousepad",
    "org.xfce.mousepad",
    "thunar",
    "org.xfce.thunar",
    "thunar-settings",
    "org.xfce.thunar-settings",
    "thunar-volman-settings",
    // Common GNOME apps that report bare Exec basenames as Wayland/X11 class.
    "seahorse",
    "rhythmbox",
    "shotwell",
    "totem",
    "eog",
    "evince",
    "simple-scan",
    "gnome-disks",
    "gnome-terminal",
    "org.gnome.terminal",
    "gnome-text-editor",
    "org.gnome.texteditor",
    "virt-manager",
    "org.vinegarhq.sober",
    "sober",
    "gnome-power-statistics",
    "gnome-control-center",
    "gnome-language-selector",
    "gnome-session-properties",
    "protontricks",
    // Ubuntu / GNOME settings helpers (bare Exec / StartupWMClass).
    "blueman",
    "blueman-manager",
    "nm-connection-editor",
    "update-manager",
    "system-software-update",
    "software-properties-gtk",
    "software-properties-drivers",
    "jockey",
    "firmware-updater",
    "firmware-updater_firmware-updater",
    "extension-manager",
    "com.mattjakeman.extensionmanager",
    // Qt config tools draw their own frame.
    "qt5ct",
    "qt6ct",
    // Common third-party / Ubuntu GTK apps (bare WM class).
    "transmission",
    "transmission-gtk",
    "remmina",
    "pavucontrol",
    "filezilla",
    "thunderbird",
    "thunderbird_thunderbird",
    "solaar",
    "qalculate-gtk",
    "qalculate",
    "xarchiver",
    "guvcview",
    "gdebi-gtk",
    "gdebi",
    "nvidia-settings",
    "usb-creator-gtk",
    "system-config-printer",
    "dbeaver",
    "dbeaver-ce",
    "zoom",
    "flatseal",
    "missioncenter",
    "io.missioncenter.missioncenter",
    "com.github.tchx84.flatseal",
    "com.github.matoking.protontricks",
    "yad",
    "yad-icon-browser",
    "hytalelauncher",
    "com.hypixel.hytalelauncher",
    // LibreOffice module WM classes.
    "libreoffice",
    "libreoffice-startcenter",
    "libreoffice-writer",
    "libreoffice-calc",
    "libreoffice-impress",
    "libreoffice-draw",
    "libreoffice-math",
    "libreoffice-base",
    "soffice",
    "org.libreoffice.libreoffice",
];

fn norm_app_id(app_id: &str) -> String {
    app_id.trim().to_lowercase()
}

fn id_matches_list(app_id: &str, list: &[&str]) -> bool {
    let id = norm_app_id(app_id);
    list.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        id == entry || id.ends_with(&format!(".{entry}"))
    })
}

/// Formerly used to skip maximize wobble on Chromium/Electron (Ozone map-origin
/// crashes). Wobble is enabled for those apps again; kept as a named helper for
/// any future per-app FX policy.
#[allow(dead_code)]
pub fn id_skips_maximize_wobble(app_id: &str) -> bool {
    let _ = app_id;
    false
}

/// True when the app id belongs to a Chromium-based browser (native CSD on Wayland).
pub fn id_looks_chromium_family(app_id: &str) -> bool {
    let id = norm_app_id(app_id);
    id.contains("chromium")
        || id == "chrome"
        || id.ends_with(".chrome")
        || id.contains("google-chrome")
        || id.contains("chrome-stable")
        || id.contains("chrome-beta")
        || id.starts_with("com.google.chrome")
        || id.starts_with("com.brave.")
        || id.contains("brave")
        || id.starts_with("com.microsoft.edge")
        || id.contains("msedge")
        || id.contains("microsoft-edge")
        || id.contains("vivaldi")
        || id.contains("opera")
}

/// Executable base names of the real Chromium-family *browsers* (which draw
/// their own window controls). Matched loosely (`contains`) so distro wrappers
/// like `chromium-browser`, `google-chrome-stable`, or `brave-browser` count.
fn exe_is_chromium_browser(exe: &str) -> bool {
    let base = exe.rsplit('/').next().unwrap_or(exe).to_lowercase();
    [
        "chrome",
        "chromium",
        "brave",
        "msedge",
        "microsoft-edge",
        "vivaldi",
        "opera",
    ]
    .iter()
    .any(|b| base.contains(b))
}

/// Frameless Electron apps (e.g. Claude Desktop) report the generic `chromium`
/// Wayland `app_id` but, unlike the real browser, draw no titlebar or controls.
/// They can't be told apart by `app_id` alone, so we use the client executable:
/// a chromium-class window whose process is **not** one of the known browsers is
/// an Electron shell that needs Metis server-side chrome.
pub fn chromium_class_needs_ssd(app_id: &str, exe: &str) -> bool {
    id_looks_chromium_family(app_id) && !exe_is_chromium_browser(exe)
}

/// True for Mozilla Firefox builds (snap `firefox_firefox`, deb `firefox`, …).
pub fn id_looks_firefox(app_id: &str) -> bool {
    norm_app_id(app_id).contains("firefox")
}

/// True for common web browsers (Firefox + Chromium-family browsers / PWAs).
///
/// Used to skip pointer-motion throttling so canvas / WebGL games stay responsive
/// when not in true fullscreen. Electron shells that report a generic `chromium`
/// app_id are included; that is intentional (also benefits from full-rate motion).
pub fn id_looks_web_browser(app_id: &str) -> bool {
    id_looks_firefox(app_id) || id_looks_chromium_family(app_id)
}

/// True when the app has no native titlebar and needs Metis chrome.
pub fn id_looks_ssd(app_id: &str) -> bool {
    id_matches_list(app_id, SSD_APP_IDS)
}

/// True when the app ships its own titlebar (GNOME/libadwaita, browsers, …).
pub fn id_looks_csd(app_id: &str) -> bool {
    if id_looks_ssd(app_id) {
        return false;
    }
    if id_looks_chromium_family(app_id) || id_looks_firefox(app_id) {
        return true;
    }
    // Thunderbird (snap `thunderbird_thunderbird`, deb `thunderbird`, …).
    if norm_app_id(app_id).contains("thunderbird") {
        return true;
    }
    // Transmission reports `Transmission` / `transmission-gtk` depending on
    // toolkit / session — accept any id containing the project name.
    if norm_app_id(app_id).contains("transmission") {
        return true;
    }
    let id = norm_app_id(app_id);
    if id_matches_list(app_id, CSD_APP_IDS) {
        return true;
    }
    if id.starts_with("org.gnome.") {
        return true;
    }
    // XFCE apps (Mousepad, Thunar, …) ship client chrome / menubars that fight
    // Metis SSD. Bare WM-class ids (`thunar`, `mousepad`) are covered by the
    // explicit CSD list; reverse-DNS still needs the prefix.
    if id.starts_with("org.xfce.") {
        return true;
    }
    if id.starts_with("org.remmina.") {
        return true;
    }
    // Wine / Proton `.exe` classes are intentionally NOT marked CSD. Many Win32
    // apps (Cheat Engine, …) set Motif "no decorations" and draw no Linux chrome,
    // so Auto leaves them frameless — Metis SSD is the right default. Users who
    // hit double-chrome on a self-decorating Wine title can Force App titlebar.
    // LibreOffice modules: reverse-DNS or `libreoffice-*` / `soffice` WM class.
    if id.starts_with("org.libreoffice.")
        || id.starts_with("libreoffice-")
        || id == "libreoffice"
        || id == "soffice"
    {
        return true;
    }
    // Ubuntu App Center / Software.
    if id.starts_with("io.snapcraft.") || id.contains("snap-store") || id.contains("snap_store") {
        return true;
    }
    if id.starts_with("io.github.")
        || id.starts_with("io.gitlab.")
        || id.starts_with("io.sourceforge.")
    {
        return true;
    }
    // Bare `electron` / `Electron` is what Claude Desktop (and other frameless
    // Electron shells) often report as Wayland app_id — they draw no titlebar
    // (`--disable-features=CustomTitlebar` / `ELECTRON_USE_SYSTEM_TITLE_BAR`).
    // Only treat ids that embed "electron" as part of a larger name (or Cursor /
    // VS Code) as native-CSD candidates.
    if id == "electron" {
        return false;
    }
    id.contains("electron") || id.contains("cursor") || id.ends_with(".code") || id == "code"
}

/// Wine / Proton X11 class (`*.exe`, `wine-*`).
pub fn id_looks_wine(app_id: &str) -> bool {
    let id = norm_app_id(app_id);
    id.ends_with(".exe") || id.starts_with("wine-")
}

/// Whether Metis should own window chrome for this client.
///
/// `user_override`: `Some(true)` force SSD, `Some(false)` force CSD, `None` Auto.
pub fn resolve_uses_ssd(
    app_id: Option<&str>,
    negotiated_mode: Option<DecorationMode>,
    decoration_bound: bool,
    user_override: Option<bool>,
) -> bool {
    // Settings / decorations.json always wins for windowed clients.
    if let Some(force_ssd) = user_override {
        return force_ssd;
    }
    // Known app classes win over xdg-decoration mode. Many GTK/headerbar clients
    // still request ServerSide while drawing their own chrome (App Center, Yelp,
    // LibreOffice, Mousepad) — honoring ServerSide first caused double titlebars.
    // Frameless Electron shells that report `chromium` + ServerSide are restored
    // to Metis SSD in `MetisState::refresh_window_decoration_mode` via
    // `chromium_class_needs_ssd` (exe disambiguation).
    if let Some(app_id) = app_id.filter(|id| !id.is_empty()) {
        if id_looks_ssd(app_id) {
            return true;
        }
        if id_looks_csd(app_id) {
            return false;
        }
    }

    match negotiated_mode {
        // Honor explicit decoration negotiation for unclassified clients.
        Some(DecorationMode::ClientSide) => false,
        Some(DecorationMode::ServerSide) => true,
        // GTK/libadwaita and Chromium bind xdg-decoration early; treat that as
        // a CSD client until app_id classifies a terminal requesting SSD.
        None if decoration_bound => false,
        None => true,
        Some(_) => true,
    }
}

/// Defer drawing Metis SSD while a headerbar app is still reporting its `app_id`.
/// Does not change `uses_ssd` — borderless clients keep default SSD once un-deferred.
pub fn defer_ssd_paint(
    app_id: Option<&str>,
    negotiated_mode: Option<DecorationMode>,
    decoration_bound: bool,
) -> bool {
    if app_id.is_some_and(|id| !id.is_empty()) {
        return false;
    }
    if negotiated_mode.is_some() {
        return false;
    }
    decoration_bound
}

/// True when auto-hide should reveal only a compact control strip (top-right)
/// instead of a full-width titlebar. Reserved for future SSD tabbed clients;
/// CSD browsers use native chrome exclusively.
pub fn id_uses_compact_overlay(_app_id: &str) -> bool {
    false
}

/// Whether an SSD-decorated window should auto-hide its Metis titlebar when
/// maximized / snapped / grid-tiled. All Metis-decorated windows use the
/// hover overlay so the client can fill the footprint; apps with top tabs only
/// see the titlebar while the pointer is in the reveal strip.
#[allow(dead_code)] // policy hook; wired when per-app titlebar rules land
pub fn id_auto_hides_titlebar(_app_id: &str) -> bool {
    true
}

/// Mode to grant over `xdg-decoration` for a window we manage.
pub fn grant_decoration_mode(uses_ssd: bool) -> DecorationMode {
    if uses_ssd {
        DecorationMode::ServerSide
    } else {
        DecorationMode::ClientSide
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_uses_native_csd() {
        assert!(!resolve_uses_ssd(
            Some("firefox_firefox"),
            None,
            false,
            None
        ));
        assert!(!resolve_uses_ssd(
            Some("org.mozilla.firefox"),
            None,
            false,
            None
        ));
        assert!(id_looks_csd("firefox_firefox"));
    }

    #[test]
    fn text_editor_keeps_csd() {
        for id in [
            "org.gnome.TextEditor",
            "gnome-text-editor",
            "org.gnome.texteditor",
        ] {
            assert!(
                !resolve_uses_ssd(Some(id), None, false, None),
                "{id} should keep client chrome under Auto"
            );
        }
    }

    #[test]
    fn virt_manager_and_sober_keep_csd() {
        for id in ["virt-manager", "org.vinegarhq.sober", "sober"] {
            assert!(
                !resolve_uses_ssd(Some(id), Some(DecorationMode::ServerSide), true, None),
                "{id} should keep client chrome under Auto"
            );
        }
    }

    #[test]
    fn chromium_uses_client_side_only() {
        assert!(!resolve_uses_ssd(
            Some("org.chromium.Chromium"),
            None,
            false,
            None
        ));
        assert!(!resolve_uses_ssd(
            Some("com.google.Chrome"),
            None,
            false,
            None
        ));
        assert!(!resolve_uses_ssd(Some("chromium"), None, false, None));
        assert!(id_looks_csd("chromium"));
    }

    /// Chromium-class apps are classified CSD by resolve; frameless Electron is
    /// forced to Metis SSD later via `chromium_class_needs_ssd` (exe check).
    #[test]
    fn chromium_stays_csd_in_resolve_even_with_server_side() {
        assert!(!resolve_uses_ssd(
            Some("chromium"),
            Some(DecorationMode::ServerSide),
            true,
            None,
        ));
        assert!(!resolve_uses_ssd(
            Some("chromium"),
            Some(DecorationMode::ClientSide),
            true,
            None,
        ));
    }

    #[test]
    fn ubuntu_settings_stragglers_keep_csd() {
        for id in [
            "eog",
            "evince",
            "blueman",
            "blueman-manager",
            "nm-connection-editor",
            "update-manager",
            "software-properties-gtk",
            "firmware-updater",
            "extension-manager",
            "com.mattjakeman.extensionmanager",
            "qt5ct",
            "gnome-terminal",
            "simple-scan",
            "thunar-volman-settings",
        ] {
            assert!(
                !resolve_uses_ssd(Some(id), Some(DecorationMode::ServerSide), true, None),
                "{id} should keep client chrome under Auto"
            );
        }
    }

    #[test]
    fn claude_and_mpv_use_metis_ssd() {
        assert!(resolve_uses_ssd(Some("claude"), None, true, None));
        assert!(resolve_uses_ssd(Some("claude-desktop"), None, true, None));
        assert!(resolve_uses_ssd(Some("mpv"), None, false, None));
        assert!(!id_looks_csd("claude"));
        assert!(!id_looks_csd("mpv"));
    }

    #[test]
    fn headerbar_apps_keep_csd_even_if_they_request_server_side() {
        for id in [
            "org.gnome.Yelp",
            "yelp",
            "io.snapcraft.Store",
            "snap-store",
            "mousepad",
            "org.xfce.mousepad",
            "thunar",
            "org.xfce.thunar",
            "libreoffice-writer",
            "org.libreoffice.LibreOffice",
            "soffice",
        ] {
            assert!(
                !resolve_uses_ssd(Some(id), Some(DecorationMode::ServerSide), true, None),
                "{id} must not get Metis SSD over its own chrome"
            );
        }
    }

    #[test]
    fn user_override_beats_heuristics_and_protocol() {
        // Force Metis SSD on a known CSD app.
        assert!(resolve_uses_ssd(
            Some("org.gnome.Cheese"),
            Some(DecorationMode::ClientSide),
            true,
            Some(true),
        ));
        // Force native CSD on a terminal.
        assert!(!resolve_uses_ssd(
            Some("kitty"),
            Some(DecorationMode::ServerSide),
            true,
            Some(false),
        ));
    }

    #[test]
    fn gnome_apps_keep_client_headerbar() {
        assert!(!resolve_uses_ssd(
            Some("org.gnome.Cheese"),
            None,
            false,
            None
        ));
        assert!(!resolve_uses_ssd(
            Some("org.gnome.Calculator"),
            None,
            false,
            None
        ));
        assert!(!resolve_uses_ssd(Some("cheese"), None, false, None));
    }

    #[test]
    fn metis_and_terminals_use_ssd() {
        assert!(resolve_uses_ssd(Some("org.kitty"), None, false, None));
        assert!(resolve_uses_ssd(
            Some("com.metis.Settings"),
            None,
            false,
            None
        ));
        // Metis helper toplevels drop their GTK titlebar, so they must keep SSD
        // even though GTK binds xdg-decoration and would otherwise fall to CSD.
        for id in ["com.metis.Screenshot", "com.metis.Viewer"] {
            assert!(resolve_uses_ssd(Some(id), None, true, None));
        }
    }

    /// Terminals report bare WM-class app_ids at runtime, and they bind
    /// xdg-decoration early — so without an explicit list entry they would hit
    /// the `decoration_bound → CSD` fallback and lose Metis chrome. Pin the real
    /// runtime ids, including with `decoration_bound = true`.
    #[test]
    fn bare_terminal_ids_use_ssd() {
        for id in [
            "kitty",
            "Alacritty",
            "alacritty",
            "foot",
            "footclient",
            "ghostty",
            "wezterm",
        ] {
            assert!(
                resolve_uses_ssd(Some(id), None, false, None),
                "{id} should use Metis SSD"
            );
            // Even after binding xdg-decoration without an explicit mode.
            assert!(
                resolve_uses_ssd(Some(id), None, true, None),
                "{id} should use Metis SSD even when decoration is bound"
            );
        }
    }

    #[test]
    fn unclassified_client_side_honored() {
        assert!(!resolve_uses_ssd(
            None,
            Some(DecorationMode::ClientSide),
            false,
            None
        ));
        assert!(resolve_uses_ssd(
            None,
            Some(DecorationMode::ServerSide),
            false,
            None
        ));
        assert!(!resolve_uses_ssd(
            Some("org.gnome.Cheese"),
            Some(DecorationMode::ClientSide),
            false,
            None,
        ));
    }

    #[test]
    fn unknown_defaults_to_ssd() {
        assert!(resolve_uses_ssd(None, None, false, None));
    }

    #[test]
    fn unknown_decoration_bound_defaults_to_csd() {
        assert!(!resolve_uses_ssd(None, None, true, None));
    }

    #[test]
    fn unknown_server_side_request_stays_ssd() {
        assert!(resolve_uses_ssd(
            None,
            Some(DecorationMode::ServerSide),
            true,
            None
        ));
    }

    #[test]
    fn client_side_protocol_disables_ssd_for_csd_apps() {
        assert!(!resolve_uses_ssd(
            Some("org.gnome.Cheese"),
            Some(DecorationMode::ClientSide),
            false,
            None,
        ));
    }

    #[test]
    fn defer_paint_for_unclassified_gtk() {
        assert!(defer_ssd_paint(None, None, true));
        assert!(!defer_ssd_paint(None, None, false));
        assert!(!defer_ssd_paint(Some("org.kitty"), None, true));
        assert!(!defer_ssd_paint(
            None,
            Some(DecorationMode::ClientSide),
            true
        ));
    }

    /// Frameless Electron apps report `chromium` but draw no chrome; a real
    /// browser process keeps native CSD. Disambiguated by the client executable.
    #[test]
    fn github_desktop_uses_metis_ssd() {
        for id in [
            "io.github.shiftey.Desktop",
            "io.github.shiftkey.GitHubDesktop",
            "com.github.GitHubDesktop",
            "GitHub Desktop",
        ] {
            assert!(
                resolve_uses_ssd(Some(id), None, false, None),
                "{id} should use Metis SSD"
            );
            assert!(!id_looks_csd(id), "{id} must not be treated as native CSD");
        }
    }

    #[test]
    fn bare_electron_uses_metis_ssd() {
        assert!(resolve_uses_ssd(Some("electron"), None, true, None));
        assert!(resolve_uses_ssd(
            Some("electron"),
            Some(DecorationMode::ServerSide),
            true,
            None
        ));
        assert!(!id_looks_csd("electron"));
        assert!(id_looks_ssd("electron"));
    }

    #[test]
    fn wine_exe_is_not_auto_csd() {
        assert!(!id_looks_csd("cheatengine-x86_64-sse4-avx2.exe"));
        assert!(id_looks_wine("cheatengine-x86_64-sse4-avx2.exe"));
        assert!(id_looks_wine("notepad.exe"));
        assert!(!id_looks_wine("kitty"));
    }

    #[test]
    fn frameless_electron_chromium_gets_metis_ssd() {
        // Claude Desktop and generic Electron shells → Metis SSD.
        assert!(chromium_class_needs_ssd(
            "chromium",
            "/usr/bin/claude-desktop"
        ));
        assert!(chromium_class_needs_ssd("chromium", "electron"));
        assert!(chromium_class_needs_ssd("chromium", "/opt/SomeApp/someapp"));
        // Real Chromium-family browsers → native CSD (never Metis SSD).
        assert!(!chromium_class_needs_ssd(
            "chromium",
            "/usr/lib/chromium/chromium"
        ));
        assert!(!chromium_class_needs_ssd(
            "chrome",
            "/opt/google/chrome/chrome"
        ));
        assert!(!chromium_class_needs_ssd(
            "chromium",
            "/usr/bin/chromium-browser"
        ));
        assert!(!chromium_class_needs_ssd(
            "com.brave.Browser",
            "brave-browser"
        ));
        // Non-chromium windows are never affected by this path.
        assert!(!chromium_class_needs_ssd("kitty", "/usr/bin/kitty"));
        assert!(!chromium_class_needs_ssd("firefox", "/usr/bin/firefox"));
    }

    #[test]
    fn web_browser_app_ids() {
        assert!(id_looks_web_browser("firefox"));
        assert!(id_looks_web_browser("org.mozilla.firefox"));
        assert!(id_looks_web_browser("firefox_firefox"));
        assert!(id_looks_web_browser("chromium"));
        assert!(id_looks_web_browser("google-chrome"));
        assert!(!id_looks_web_browser("org.kitty"));
        assert!(!id_looks_web_browser("org.gnome.Nautilus"));
    }

    #[test]
    fn maximize_wobble_enabled_for_chromium_family() {
        // Wobble is no longer skipped for Ozone/Electron shells.
        assert!(!id_skips_maximize_wobble("chromium"));
        assert!(!id_skips_maximize_wobble("org.chromium.Chromium"));
        assert!(!id_skips_maximize_wobble("cursor"));
        assert!(!id_skips_maximize_wobble("org.kitty"));
    }

    #[test]
    fn no_compact_overlay_apps() {
        assert!(!id_uses_compact_overlay("chromium"));
        assert!(!id_uses_compact_overlay("firefox_firefox"));
        assert!(!id_uses_compact_overlay("org.kitty"));
    }

    #[test]
    fn all_ssd_apps_auto_hide_titlebar() {
        assert!(id_auto_hides_titlebar("org.kitty"));
        assert!(id_auto_hides_titlebar("com.metis.Settings"));
    }

    #[test]
    fn decoration_bound_does_not_disable_ssd_in_resolve() {
        assert!(resolve_uses_ssd(None, None, false, None));
        assert!(!resolve_uses_ssd(None, None, true, None));
    }
}
