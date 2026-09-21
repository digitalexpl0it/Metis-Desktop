use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BarPosition {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

/// Which outputs (monitors) the edge bar appears on in a multi-monitor session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BarDisplays {
    /// One bar per connected output (default).
    #[default]
    All,
    /// A single bar on the primary output only.
    Primary,
}

/// How virtual workspaces behave across multiple outputs (monitors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    /// Each output owns an independent set of workspaces; switching one output's
    /// workspace leaves the others alone.
    Separate,
    /// All outputs switch together: changing to workspace N moves every monitor to
    /// its own workspace N at once (default).
    #[default]
    Linked,
}

/// The layout mode new workspaces start in. Mirrors `metis_grid::LayoutKind`
/// without pulling the grid crate into config; the compositor maps between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DefaultLayout {
    /// Regular floating desktop (default).
    #[default]
    Free,
    /// Auto-tiling grid below desk widgets.
    Grid,
    /// A horizontally scrolling strip of columns (niri / PaperWM style).
    Scroll,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockConfig {
    #[serde(default = "default_time_format")]
    pub time_format: String,
    #[serde(default = "default_date_format")]
    pub date_format: String,
    #[serde(default)]
    pub timezones: Vec<String>,
}

fn default_time_format() -> String {
    "%I:%M %p".into()
}

fn default_date_format() -> String {
    "%a %b %d".into()
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            time_format: default_time_format(),
            date_format: default_date_format(),
            timezones: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarWidgetId {
    Workspaces,
    Tasks,
    Spacer,
    Clock,
    Battery,
    Network,
    /// NetworkManager VPN / WireGuard profiles (connect / disconnect popover).
    Vpn,
    Bluetooth,
    Volume,
    Notifications,
    /// Clipboard history (text + image previews).
    Clipboard,
    Weather,
    /// Removable USB / SD / optical / ISO volumes (Gio VolumeMonitor).
    RemovableVolumes,
    /// System tray (StatusNotifierItem) host.
    Tray,
}

/// How system tray app icons appear on the edge bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrayIconMode {
    /// One tray button opens a popover listing all tray app icons (default).
    #[default]
    Collapsed,
    /// Tray app icons are pinned inline on the bar, to the left of the tray button.
    Pinned,
}

/// Transparent padding baked into the bar's layer surface (beyond the visible
/// pill) so the pill's rounded drop shadow renders without being clipped square.
/// `SHADOW_PAD` is on the inner edge (below a top bar); `PILL_SIDE_INSET` is on
/// the two long edges. The compositor uses these to confine backdrop effects
/// (e.g. blur) to the visible pill and exclude the shadow margin.
pub const SHADOW_PAD: i32 = 16;
pub const PILL_SIDE_INSET: i32 = SHADOW_PAD - 4;

/// Inner-edge shadow pad baked into the layer surface.
///
/// Zero only when distance is 0 so the bar can sit truly flush to the anchored
/// screen edge. At distance > 0 the pad sits on the *inner* side (toward the
/// desktop) and must not be mistaken for edge distance — distance is solely
/// `margin_top` on the layer-shell margin.
pub fn bar_layer_shadow_pad(cfg: &BarConfig) -> i32 {
    if cfg.margin_top == 0 {
        0
    } else {
        SHADOW_PAD
    }
}

/// Side inset for the pill within the layer so stadium/rounded ends and their
/// drop shadow are not clipped. Always applied (including distance 0) — distance
/// 0 only flushes the anchored edge, not the rounded ends.
pub fn bar_pill_side_inset(_cfg: &BarConfig) -> i32 {
    PILL_SIDE_INSET
}

/// Layer-shell margin from the anchored screen edge to the inner side of the bar
/// pill (not including shadow pad). Used to attach the control center flush below
/// the visible bar strip.
pub fn bar_pill_inset(cfg: &BarConfig) -> i32 {
    cfg.margin_top as i32 + cfg.height as i32
}

/// Maximized / snapped window edge padding from `bar.json`, clamped to 0..=10.
pub fn window_gap_px(cfg: &BarConfig) -> i32 {
    cfg.window_gap_px.min(10) as i32
}

/// Control-center attach inset: tuck slightly under the bar pill so no desktop
/// gap shows through the bar's shadow pad region.
pub fn dashboard_attach_inset(cfg: &BarConfig) -> i32 {
    // Overlap a few px into the bar pill bottom edge for a seamless pull-down.
    const FLUSH_OVERLAP: i32 = 4;
    bar_pill_inset(cfg).saturating_sub(FLUSH_OVERLAP)
}

/// Notification Center attach inset: distance from the output top edge to the
/// panel top. Requires the NC layer to use exclusive_zone(-1) (Don'tCare) so
/// this margin is not stacked on top of the bar's exclusive zone.
pub fn notification_center_attach_inset(cfg: &BarConfig) -> i32 {
    // 1px below the pill bottom so the panel does not paint into the bar edge.
    bar_pill_inset(cfg).saturating_add(1)
}

/// How the compositor strokes an accent border (title pill or window frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BorderMode {
    /// Follow the active theme's accent gradient (auto-tracks light/dark).
    #[default]
    Accent,
    /// A single flat color (`color`).
    Solid,
    /// A custom gradient across `gradient`'s stops.
    Gradient,
}

/// Appearance of the thin accent border around the compositor-drawn title pill.
/// Consumed by the compositor (via `bar.json`) and edited by the settings app's
/// Appearance page. The border only paints on the *focused* window; unfocused
/// windows always use a muted slate stroke. The pill gradient flows left→right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TitlebarPillBorder {
    #[serde(default)]
    pub mode: BorderMode,
    /// Flat stroke color (`#rrggbb`) used when `mode = solid`.
    #[serde(default = "default_pill_color")]
    pub color: String,
    /// Gradient stops (`#rrggbb`), used when `mode = gradient`. Two or more
    /// stops recommended.
    #[serde(default = "default_pill_gradient")]
    pub gradient: Vec<String>,
    /// Stroke thickness in pixels.
    #[serde(default = "default_pill_border_width")]
    pub width_px: f32,
}

/// Appearance of the compositor-drawn window frame border (the left/right/bottom
/// edges + the titlebar ring). Independent of the title pill. The frame gradient
/// flows top→bottom; `width_px` sets the frame thickness and insets the client body
/// to match. Focused windows draw this stroke; unfocused use a muted slate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowBorder {
    #[serde(default)]
    pub mode: BorderMode,
    /// Flat stroke color (`#rrggbb`) used when `mode = solid`.
    #[serde(default = "default_window_border_color")]
    pub color: String,
    /// Gradient stops (`#rrggbb`), used when `mode = gradient`.
    #[serde(default = "default_window_border_gradient")]
    pub gradient: Vec<String>,
    /// Frame thickness in pixels (0–16). Insets the client body to match.
    #[serde(default = "default_window_border_width")]
    pub width_px: f32,
}

/// Appearance of the border drawn around the edge bar's pill, rendered by the
/// shell via GTK CSS. Independent of the window/title-pill borders. `accent`
/// follows the theme accent gradient; `gradient` uses custom stops; the gradient
/// flows along the bar's long axis (left→right when horizontal, top→bottom when
/// vertical). `width_px = 0` disables the border entirely.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarBorder {
    #[serde(default)]
    pub mode: BorderMode,
    /// Flat stroke color (`#rrggbb`) used when `mode = solid`.
    #[serde(default = "default_bar_border_color")]
    pub color: String,
    /// Gradient stops (`#rrggbb`), used when `mode = gradient`.
    #[serde(default = "default_bar_border_gradient")]
    pub gradient: Vec<String>,
    /// Border thickness in pixels (0 disables the border).
    #[serde(default = "default_bar_border_width")]
    pub width_px: f32,
}

/// How the edge bar fills its pill background (independent of [`BarBorder`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BarFillMode {
    /// Theme surface token (default).
    #[default]
    Theme,
    /// Flat `color`.
    Solid,
    /// Custom `gradient` stops.
    Gradient,
}

/// CSS linear-gradient direction for [`BarFill`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BarGradientDirection {
    /// Along the bar's long axis (left→right when horizontal, top→bottom when vertical).
    #[default]
    Auto,
    ToRight,
    ToLeft,
    ToBottom,
    ToTop,
}

/// Edge bar pill fill — theme surface, solid color, or gradient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarFill {
    #[serde(default)]
    pub mode: BarFillMode,
    #[serde(default = "default_bar_fill_color")]
    pub color: String,
    #[serde(default = "default_bar_fill_gradient")]
    pub gradient: Vec<String>,
    #[serde(default)]
    pub gradient_direction: BarGradientDirection,
}

fn default_bar_fill_color() -> String {
    "#1a1b26".into()
}

fn default_bar_fill_gradient() -> Vec<String> {
    vec!["#1a1b26".into(), "#2a2b3d".into()]
}

impl Default for BarFill {
    fn default() -> Self {
        Self {
            mode: BarFillMode::Theme,
            color: default_bar_fill_color(),
            gradient: default_bar_fill_gradient(),
            gradient_direction: BarGradientDirection::Auto,
        }
    }
}

fn default_bar_border_color() -> String {
    "#3d3846".into()
}

fn default_bar_border_gradient() -> Vec<String> {
    vec!["#00F2FE".into(), "#4FACFE".into(), "#A24BFF".into()]
}

fn default_bar_border_width() -> f32 {
    0.0
}

impl Default for BarBorder {
    fn default() -> Self {
        Self {
            mode: BorderMode::Solid,
            color: default_bar_border_color(),
            gradient: default_bar_border_gradient(),
            width_px: default_bar_border_width(),
        }
    }
}

fn default_pill_color() -> String {
    "#000000".into()
}

fn default_pill_gradient() -> Vec<String> {
    vec!["#00F2FE".into(), "#4FACFE".into(), "#A24BFF".into()]
}

fn default_pill_border_width() -> f32 {
    0.0
}

fn default_window_border_color() -> String {
    "#77767b".into()
}

fn default_window_border_gradient() -> Vec<String> {
    vec!["#00F2FE".into(), "#4FACFE".into(), "#A24BFF".into()]
}

fn default_window_border_width() -> f32 {
    0.0
}

impl Default for TitlebarPillBorder {
    fn default() -> Self {
        Self {
            mode: BorderMode::Solid,
            color: default_pill_color(),
            gradient: default_pill_gradient(),
            width_px: default_pill_border_width(),
        }
    }
}

impl Default for WindowBorder {
    fn default() -> Self {
        Self {
            mode: BorderMode::Solid,
            color: default_window_border_color(),
            gradient: default_window_border_gradient(),
            width_px: default_window_border_width(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BarConfig {
    #[serde(default)]
    pub position: BarPosition,
    /// Which outputs the bar appears on (all monitors vs. primary only).
    #[serde(default)]
    pub displays: BarDisplays,
    #[serde(default = "default_height")]
    pub height: u32,
    /// Legacy field; vertical bars use `height` for cross-axis thickness so the
    /// strip matches a horizontal top/bottom bar. Kept for config compatibility.
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_margin_top")]
    pub margin_top: u32,
    #[serde(default = "default_margin_h")]
    pub margin_h: u32,
    #[serde(default = "default_full_width")]
    pub full_width: bool,
    /// Along-edge length as a percent of the screen edge (40–100). Always
    /// centered. `100` is a full-edge strip (subject to [`Self::full_width`] for
    /// legacy content-hug when false).
    #[serde(default = "default_length_percent")]
    pub length_percent: u32,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// Background opacity for the start-menu popover (text/icons stay opaque).
    /// Applied by the shell as a CSS override, mirroring `opacity` for the bar.
    #[serde(default = "default_menu_opacity")]
    pub menu_opacity: f32,
    /// Background opacity for compositor-drawn window titlebars (title text and
    /// the traffic-light buttons stay opaque). Consumed by the compositor.
    #[serde(default = "default_titlebar_opacity")]
    pub titlebar_opacity: f32,
    /// Appearance of the thin accent border around window title pills. Consumed by
    /// the compositor.
    #[serde(default)]
    pub titlebar_pill_border: TitlebarPillBorder,
    /// Appearance + thickness of the window frame border. Consumed by the compositor;
    /// `width_px` also insets the client body.
    #[serde(default)]
    pub window_border: WindowBorder,
    /// Appearance + thickness of the border around the edge bar's pill. Consumed by
    /// the shell (rendered via GTK CSS); `width_px = 0` disables it.
    #[serde(default)]
    pub bar_border: BarBorder,
    /// Pill fill (theme surface / solid / gradient). Consumed by the shell.
    #[serde(default)]
    pub bar_fill: BarFill,
    /// Slide the bar off-edge after idle, leaving a peek strip.
    #[serde(default)]
    pub auto_hide: bool,
    /// Idle delay before auto-hide (ms). Kept for config compat; hide is instant
    /// when the pointer leaves (slide animation is CSS-only).
    #[serde(default = "default_auto_hide_delay_ms")]
    pub auto_hide_delay_ms: u32,
    /// Visible peek thickness when auto-hidden (px).
    #[serde(default = "default_auto_hide_peek_px")]
    pub auto_hide_peek_px: u32,
    #[serde(default = "default_true")]
    pub blur: bool,
    /// Gaussian backdrop-blur radius (in pixels) applied by the compositor behind
    /// the bar when `blur` is enabled. Consumed by the compositor via bar.json.
    #[serde(default = "default_blur_radius")]
    pub blur_radius: f32,
    /// When false, compositor window effects (minimize genie, maximize wobble,
    /// titlebar slide) run instantly.
    #[serde(default = "default_true")]
    pub window_animations: bool,
    /// Padding (logical px) around maximized / edge-snapped windows inside the
    /// usable area. `0` is flush to the screen/bar edges; max is 10.
    #[serde(default = "default_window_gap_px")]
    pub window_gap_px: u32,
    /// How StatusNotifier tray icons are shown on the edge bar.
    #[serde(default)]
    pub tray_icon_mode: TrayIconMode,
    #[serde(default = "default_widgets")]
    pub widgets: Vec<BarWidgetId>,
    #[serde(default)]
    pub clock: ClockConfig,
    /// Number of workspace indicator dots (1–12).
    #[serde(default = "default_workspace_count")]
    pub workspace_count: u32,
    /// How workspaces behave across multiple monitors (independent vs. linked).
    #[serde(default)]
    pub workspace_mode: WorkspaceMode,
    /// Layout mode new workspaces start in (grid tiling vs. scrolling strip).
    #[serde(default)]
    pub default_layout: DefaultLayout,
    /// App ids pinned to the taskbar/dock, in display order. Independent of the
    /// launcher's `menu.json` pins. Persisted by the tasks widget.
    #[serde(default)]
    pub taskbar_pinned: Vec<String>,
}

fn default_workspace_count() -> u32 {
    4
}

fn default_height() -> u32 {
    36
}

fn default_width() -> u32 {
    48
}

fn default_margin_top() -> u32 {
    1
}

fn default_margin_h() -> u32 {
    10
}

fn default_full_width() -> bool {
    true
}

fn default_length_percent() -> u32 {
    100
}

fn default_auto_hide_delay_ms() -> u32 {
    0
}

fn default_auto_hide_peek_px() -> u32 {
    2
}

fn default_opacity() -> f32 {
    0.61
}

fn default_menu_opacity() -> f32 {
    0.92
}

fn default_titlebar_opacity() -> f32 {
    0.72
}

fn default_blur_radius() -> f32 {
    4.0
}

fn default_window_gap_px() -> u32 {
    0
}

fn default_true() -> bool {
    true
}

fn default_widgets() -> Vec<BarWidgetId> {
    vec![
        BarWidgetId::Workspaces,
        BarWidgetId::Tasks,
        BarWidgetId::Spacer,
        BarWidgetId::RemovableVolumes,
        BarWidgetId::Tray,
        BarWidgetId::Weather,
        BarWidgetId::Battery,
        BarWidgetId::Network,
        BarWidgetId::Vpn,
        BarWidgetId::Bluetooth,
        BarWidgetId::Volume,
        BarWidgetId::Clipboard,
        BarWidgetId::Clock,
    ]
}

impl Default for BarConfig {
    fn default() -> Self {
        Self {
            position: BarPosition::Top,
            displays: BarDisplays::default(),
            height: default_height(),
            width: default_width(),
            margin_top: default_margin_top(),
            margin_h: default_margin_h(),
            full_width: default_full_width(),
            length_percent: default_length_percent(),
            opacity: default_opacity(),
            menu_opacity: default_menu_opacity(),
            titlebar_opacity: default_titlebar_opacity(),
            titlebar_pill_border: TitlebarPillBorder::default(),
            window_border: WindowBorder::default(),
            bar_border: BarBorder::default(),
            bar_fill: BarFill::default(),
            auto_hide: false,
            auto_hide_delay_ms: default_auto_hide_delay_ms(),
            auto_hide_peek_px: default_auto_hide_peek_px(),
            blur: default_true(),
            blur_radius: default_blur_radius(),
            window_animations: default_true(),
            window_gap_px: default_window_gap_px(),
            tray_icon_mode: TrayIconMode::default(),
            widgets: default_widgets(),
            clock: ClockConfig::default(),
            workspace_count: default_workspace_count(),
            workspace_mode: WorkspaceMode::default(),
            default_layout: DefaultLayout::default(),
            taskbar_pinned: Vec::new(),
        }
    }
}

pub fn bar_config_path() -> std::path::PathBuf {
    super::config_dir().join("bar.json")
}

pub fn load_bar_config() -> BarConfig {
    let path = bar_config_path();
    let mut cfg = if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(parsed) = serde_json::from_str(&text) {
                parsed
            } else {
                tracing::warn!("bar.json parse failed — using defaults");
                BarConfig::default()
            }
        } else {
            BarConfig::default()
        }
    } else {
        BarConfig::default()
    };
    migrate_bar_config(&mut cfg);
    sanitize_bar_config(&mut cfg);
    cfg
}

/// Clamp length / auto-hide / fill fields to safe ranges.
pub fn sanitize_bar_config(cfg: &mut BarConfig) {
    cfg.length_percent = cfg.length_percent.clamp(40, 100);
    cfg.auto_hide_delay_ms = cfg.auto_hide_delay_ms.min(5000);
    cfg.auto_hide_peek_px = cfg.auto_hide_peek_px.clamp(2, 8);
    if !looks_like_hex_color(&cfg.bar_fill.color) {
        cfg.bar_fill.color = default_bar_fill_color();
    }
    cfg.bar_fill.gradient.retain(|s| looks_like_hex_color(s));
    if cfg.bar_fill.gradient.len() < 2 {
        cfg.bar_fill.gradient = default_bar_fill_gradient();
    }
    if cfg.bar_fill.gradient.len() > 8 {
        cfg.bar_fill.gradient.truncate(8);
    }
}

fn looks_like_hex_color(s: &str) -> bool {
    let s = s.trim();
    let body = s.strip_prefix('#').unwrap_or(s);
    matches!(body.len(), 3 | 6 | 8) && body.chars().all(|c| c.is_ascii_hexdigit())
}

/// Upgrade layouts saved before the eww-style pill redesign.
fn migrate_bar_config(cfg: &mut BarConfig) {
    let legacy = [
        BarWidgetId::Workspaces,
        BarWidgetId::Spacer,
        BarWidgetId::Clock,
        BarWidgetId::Spacer,
        BarWidgetId::Battery,
        BarWidgetId::Network,
        BarWidgetId::Bluetooth,
        BarWidgetId::Volume,
        BarWidgetId::Notifications,
    ];
    let center_notif = [
        BarWidgetId::Workspaces,
        BarWidgetId::Spacer,
        BarWidgetId::Notifications,
        BarWidgetId::Spacer,
        BarWidgetId::Battery,
        BarWidgetId::Network,
        BarWidgetId::Bluetooth,
        BarWidgetId::Volume,
        BarWidgetId::Clock,
    ];
    // Do not treat `margin_top == 0` as legacy — Settings "distance" 0 is valid
    // (flush to the screen edge). Resetting it to the default made bottom bars
    // look permanently inset and undid maximize reserve math.
    let needs_layout_refresh = cfg.widgets == legacy
        || cfg.widgets == center_notif
        || cfg.margin_h >= 48
        || cfg.height < 36;
    if needs_layout_refresh {
        cfg.widgets = default_widgets();
        cfg.height = default_height();
        cfg.margin_top = default_margin_top();
        cfg.margin_h = default_margin_h();
        cfg.full_width = default_full_width();
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }
    if cfg.clock.time_format == "%H:%M" {
        cfg.clock.time_format = default_time_format();
    }

    // Insert the weather widget into pre-existing layouts that predate it, ahead
    // of the system/clock cluster so it leads the right-hand group.
    if !cfg.widgets.contains(&BarWidgetId::Weather) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| {
                matches!(
                    w,
                    BarWidgetId::Battery
                        | BarWidgetId::Network
                        | BarWidgetId::Vpn
                        | BarWidgetId::Bluetooth
                        | BarWidgetId::Volume
                        | BarWidgetId::Clipboard
                        | BarWidgetId::Notifications
                        | BarWidgetId::Clock
                )
            })
            .unwrap_or(cfg.widgets.len());
        cfg.widgets.insert(pos, BarWidgetId::Weather);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }

    // Insert the taskbar/dock into pre-existing layouts that predate it, just
    // after the workspaces cluster on the left (or at the front otherwise).
    if !cfg.widgets.contains(&BarWidgetId::Tasks) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Workspaces))
            .map(|i| i + 1)
            .unwrap_or(0);
        cfg.widgets.insert(pos, BarWidgetId::Tasks);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }

    // Insert the Bluetooth indicator after Network in layouts that predate it.
    if !cfg.widgets.contains(&BarWidgetId::Bluetooth) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Network))
            .map(|i| i + 1)
            .unwrap_or(cfg.widgets.len());
        cfg.widgets.insert(pos, BarWidgetId::Bluetooth);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }

    // VPN sits immediately after Network (before Bluetooth when present).
    if !cfg.widgets.contains(&BarWidgetId::Vpn) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Network))
            .map(|i| i + 1)
            .unwrap_or(cfg.widgets.len());
        cfg.widgets.insert(pos, BarWidgetId::Vpn);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }

    // Insert the clipboard history widget before notifications in older layouts.
    if !cfg.widgets.contains(&BarWidgetId::Clipboard) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Notifications))
            .unwrap_or(cfg.widgets.len());
        cfg.widgets.insert(pos, BarWidgetId::Clipboard);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }

    // Insert the system tray widget immediately left of the weather cluster.
    if !cfg.widgets.contains(&BarWidgetId::Tray) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Weather))
            .unwrap_or(cfg.widgets.len());
        cfg.widgets.insert(pos, BarWidgetId::Tray);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    } else {
        // Reposition tray if it was placed elsewhere in an older layout.
        let weather_pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Weather));
        let tray_pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Tray));
        if let (Some(wpos), Some(tpos)) = (weather_pos, tray_pos) {
            if tpos != wpos.saturating_sub(1) {
                cfg.widgets.remove(tpos);
                let insert_at = cfg
                    .widgets
                    .iter()
                    .position(|w| matches!(w, BarWidgetId::Weather))
                    .unwrap_or(cfg.widgets.len());
                cfg.widgets.insert(insert_at, BarWidgetId::Tray);
                if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
                    let _ = std::fs::write(bar_config_path(), json);
                }
            }
        }
    }

    // Removable volumes sit immediately left of the tray.
    if !cfg.widgets.contains(&BarWidgetId::RemovableVolumes) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Tray))
            .unwrap_or_else(|| {
                cfg.widgets
                    .iter()
                    .position(|w| matches!(w, BarWidgetId::Weather))
                    .unwrap_or(cfg.widgets.len())
            });
        cfg.widgets.insert(pos, BarWidgetId::RemovableVolumes);
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    } else if let (Some(tray_pos), Some(vol_pos)) = (
        cfg.widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Tray)),
        cfg.widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::RemovableVolumes)),
    ) {
        if vol_pos != tray_pos.saturating_sub(1) {
            cfg.widgets.remove(vol_pos);
            let insert_at = cfg
                .widgets
                .iter()
                .position(|w| matches!(w, BarWidgetId::Tray))
                .unwrap_or(cfg.widgets.len());
            cfg.widgets.insert(insert_at, BarWidgetId::RemovableVolumes);
            if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
                let _ = std::fs::write(bar_config_path(), json);
            }
        }
    }

    // Phase 13: Notification Center merges the bell into the clock. Strip the
    // standalone notifications widget when the clock is present so upgrades get
    // the Win11-style single affordance.
    if cfg.widgets.contains(&BarWidgetId::Clock)
        && cfg.widgets.contains(&BarWidgetId::Notifications)
    {
        cfg.widgets
            .retain(|w| !matches!(w, BarWidgetId::Notifications));
        if let Ok(json) = serde_json::to_string_pretty(&*cfg) {
            let _ = std::fs::write(bar_config_path(), json);
        }
    }
}

pub fn save_default_bar_config() -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let path = bar_config_path();
    if path.exists() {
        return Ok(());
    }
    let json =
        serde_json::to_string_pretty(&BarConfig::default()).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Persist a full bar configuration (used by the settings app's Appearance page
/// for opacity/blur edits). The shell's `watch_bar_config` re-applies it live.
pub fn save_bar_config(config: &BarConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let mut clean = config.clone();
    sanitize_bar_config(&mut clean);
    let json = serde_json::to_string_pretty(&clean).map_err(std::io::Error::other)?;
    let path = bar_config_path();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(tmp, path)
}
