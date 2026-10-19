use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Hot-path cache: compositor housekeeping used to `read_to_string`+parse
/// `bar.json` dozens of times per second. Invalidate on mtime change / save.
static BAR_CONFIG_CACHE: Mutex<Option<(Option<SystemTime>, BarConfig)>> = Mutex::new(None);

static MIGRATE_PERSISTED: AtomicBool = AtomicBool::new(false);

fn bar_config_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn invalidate_bar_config_cache() {
    if let Ok(mut guard) = BAR_CONFIG_CACHE.lock() {
        *guard = None;
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Software updates available (PackageKit / Flatpak / fwupd). Hidden when idle.
    Updates,
}

impl BarWidgetId {
    /// Status-cluster icons the user may hide from Settings / on-bar layout.
    /// Workspaces, Tasks, Spacer, and Clock stay required.
    pub fn is_status_optional(self) -> bool {
        matches!(
            self,
            Self::Battery
                | Self::Network
                | Self::Vpn
                | Self::Bluetooth
                | Self::Volume
                | Self::Clipboard
                | Self::Weather
                | Self::RemovableVolumes
                | Self::Tray
                | Self::Updates
        )
    }

    /// Default order for optional status icons (Settings toggle catalogue).
    pub fn status_optional_catalogue() -> &'static [Self] {
        &[
            Self::RemovableVolumes,
            Self::Tray,
            Self::Updates,
            Self::Weather,
            Self::Battery,
            Self::Network,
            Self::Vpn,
            Self::Bluetooth,
            Self::Volume,
            Self::Clipboard,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Workspaces => "Workspaces",
            Self::Tasks => "Running apps",
            Self::Spacer => "Spacer",
            Self::Clock => "Clock",
            Self::Battery => "Battery",
            Self::Network => "Network",
            Self::Vpn => "VPN",
            Self::Bluetooth => "Bluetooth",
            Self::Volume => "Volume",
            Self::Notifications => "Notifications",
            Self::Clipboard => "Clipboard",
            Self::Weather => "Weather",
            Self::RemovableVolumes => "Removable volumes",
            Self::Tray => "System tray",
            Self::Updates => "Software updates",
        }
    }
}

/// Split `widgets` into required prefix, optional status segment, and whether Clock is present.
pub fn split_bar_widget_layout(
    widgets: &[BarWidgetId],
) -> (Vec<BarWidgetId>, Vec<BarWidgetId>, bool) {
    let mut status = Vec::new();
    let mut has_clock = false;
    for w in widgets {
        if *w == BarWidgetId::Clock {
            has_clock = true;
        } else if w.is_status_optional() && !status.contains(w) {
            status.push(*w);
        }
    }
    let prefix = vec![
        BarWidgetId::Workspaces,
        BarWidgetId::Tasks,
        BarWidgetId::Spacer,
    ];
    (prefix, status, has_clock)
}

/// Rebuild `widgets` as `[Workspaces, Tasks, Spacer, …status…, Clock]`.
/// Drops unknowns / Notifications; keeps only optional ids from `status`.
pub fn rebuild_bar_widgets(status: &[BarWidgetId]) -> Vec<BarWidgetId> {
    let mut out = vec![
        BarWidgetId::Workspaces,
        BarWidgetId::Tasks,
        BarWidgetId::Spacer,
    ];
    for w in status {
        if w.is_status_optional() && !out.contains(w) {
            out.push(*w);
        }
    }
    out.push(BarWidgetId::Clock);
    out
}

/// Insert `id` into the status segment (before Clock) if missing, using catalogue order
/// relative to already-enabled peers when possible.
pub fn enable_status_widget(widgets: &mut Vec<BarWidgetId>, id: BarWidgetId) {
    if !id.is_status_optional() || widgets.contains(&id) {
        return;
    }
    let (_, mut status, _) = split_bar_widget_layout(widgets);
    let catalogue = BarWidgetId::status_optional_catalogue();
    let want_idx = catalogue
        .iter()
        .position(|c| *c == id)
        .unwrap_or(status.len());
    let mut insert_at = status.len();
    for (i, existing) in status.iter().enumerate() {
        let existing_idx = catalogue
            .iter()
            .position(|c| c == existing)
            .unwrap_or(usize::MAX);
        if existing_idx > want_idx {
            insert_at = i;
            break;
        }
    }
    status.insert(insert_at, id);
    *widgets = rebuild_bar_widgets(&status);
}

/// Remove an optional status widget. No-op for required ids.
pub fn disable_status_widget(widgets: &mut Vec<BarWidgetId>, id: BarWidgetId) {
    if !id.is_status_optional() {
        return;
    }
    widgets.retain(|w| *w != id);
    let (_, status, _) = split_bar_widget_layout(widgets);
    *widgets = rebuild_bar_widgets(&status);
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
    if cfg.margin_top == 0 { 0 } else { SHADOW_PAD }
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
        BarWidgetId::Updates,
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
    let mtime = bar_config_mtime(&path);
    if let Ok(guard) = BAR_CONFIG_CACHE.lock()
        && let Some((cached_mtime, cfg)) = guard.as_ref()
        && *cached_mtime == mtime
    {
        return cfg.clone();
    }

    let mut parsed_from_disk = false;
    let mut cfg = if path.exists() {
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        // Concurrent migrate/save can briefly truncate the file; retry once so we
        // do not fall through to Default and persist an empty `taskbar_pinned`.
        if text.trim().is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(40));
            text = std::fs::read_to_string(&path).unwrap_or_default();
        }
        if text.trim().is_empty() {
            tracing::warn!("bar.json empty during read — skipping migrate persist");
            BarConfig::default()
        } else {
            match serde_json::from_str::<BarConfig>(&text) {
                Ok(parsed) => {
                    parsed_from_disk = true;
                    parsed
                }
                Err(err) => {
                    tracing::warn!(%err, "bar.json parse failed — using defaults (not rewriting disk)");
                    BarConfig::default()
                }
            }
        }
    } else {
        BarConfig::default()
    };
    migrate_bar_config(&mut cfg, parsed_from_disk);
    sanitize_bar_config(&mut cfg);
    if let Ok(mut guard) = BAR_CONFIG_CACHE.lock() {
        *guard = Some((mtime, cfg.clone()));
    }
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

/// Persist bar config after a migrate step.
///
/// Re-reads `taskbar_pinned` from disk so a concurrent dock pin/unpin (or a
/// stale in-memory empty list) cannot wipe the user's pinned apps when we only
/// meant to tweak `widgets`.
fn persist_migrated_bar(cfg: &mut BarConfig) {
    if let Ok(text) = std::fs::read_to_string(bar_config_path())
        && let Ok(disk) = serde_json::from_str::<BarConfig>(&text)
        && cfg.taskbar_pinned.is_empty()
        && !disk.taskbar_pinned.is_empty()
    {
        cfg.taskbar_pinned = disk.taskbar_pinned;
    }
    // Recovery: if the dock pin list is empty but the Metis Menu still has
    // pinned apps, seed the dock once (migrate wipe / lost bar.json).
    if cfg.taskbar_pinned.is_empty() {
        let menu_pins = crate::menu::load_menu_config().pinned;
        if !menu_pins.is_empty() {
            cfg.taskbar_pinned = menu_pins;
        }
    }
    sanitize_bar_config(cfg);
    if crate::persist::write_json_atomic(&bar_config_path(), &*cfg).is_ok() {
        invalidate_bar_config_cache();
    }
}

/// Upgrade layouts saved before the eww-style pill redesign.
///
/// `allow_persist` is false when the in-memory config did not come from a
/// successful on-disk parse — never rewrite `bar.json` from that fallback, or
/// we can wipe `taskbar_pinned` during a concurrent write race.
fn migrate_bar_config(cfg: &mut BarConfig, allow_persist: bool) {
    let mut dirty = false;
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
        dirty = true;
    }
    if cfg.clock.time_format == "%H:%M" {
        cfg.clock.time_format = default_time_format();
    }

    // Insert the taskbar/dock into pre-existing layouts that predate it, just
    // after the workspaces cluster on the left (or at the front otherwise).
    // Optional status icons are never force-inserted — Settings toggles must stick.
    if !cfg.widgets.contains(&BarWidgetId::Tasks) {
        let pos = cfg
            .widgets
            .iter()
            .position(|w| matches!(w, BarWidgetId::Workspaces))
            .map(|i| i + 1)
            .unwrap_or(0);
        cfg.widgets.insert(pos, BarWidgetId::Tasks);
        dirty = true;
    }

    // Tray / volumes / updates adjacency is no longer force-fixed — users reorder
    // status icons freely (Settings arrows + on-bar drag).

    // Phase 13: Notification Center merges the bell into the clock. Strip the
    // standalone notifications widget when the clock is present so upgrades get
    // the Win11-style single affordance.
    if cfg.widgets.contains(&BarWidgetId::Clock)
        && cfg.widgets.contains(&BarWidgetId::Notifications)
    {
        cfg.widgets
            .retain(|w| !matches!(w, BarWidgetId::Notifications));
        dirty = true;
    }

    if dirty && allow_persist {
        // At most one migrate write per process. Every write bumps the mtime,
        // which re-triggers the shell's bar rebuild and the compositor reflow;
        // a non-convergent rule would otherwise rewrite bar.json forever.
        if MIGRATE_PERSISTED.swap(true, Ordering::AcqRel) {
            tracing::warn!("bar.json migrate did not converge — not rewriting again");
        } else {
            persist_migrated_bar(cfg);
        }
    }
}

pub fn save_default_bar_config() -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let path = bar_config_path();
    if path.exists() {
        return Ok(());
    }
    crate::persist::write_json_atomic(&path, &BarConfig::default())
}

/// Persist a full bar configuration (used by the settings app's Appearance page
/// for opacity/blur edits). The shell's `watch_bar_config` re-applies it live.
pub fn save_bar_config(config: &BarConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let mut clean = config.clone();
    sanitize_bar_config(&mut clean);
    crate::persist::write_json_atomic(&bar_config_path(), &clean)?;
    invalidate_bar_config_cache();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated(widgets: Vec<BarWidgetId>) -> Vec<BarWidgetId> {
        let mut cfg = BarConfig {
            widgets,
            ..BarConfig::default()
        };
        migrate_bar_config(&mut cfg, false);
        cfg.widgets
    }

    #[test]
    fn default_layout_is_a_migrate_fixed_point() {
        let defaults = default_widgets();
        assert_eq!(migrated(defaults.clone()), defaults);
    }

    #[test]
    fn migrate_preserves_user_status_order() {
        use BarWidgetId::*;
        // Formerly migrate forced vol→tray→updates→weather; free reorder must stick.
        let input = vec![
            Workspaces,
            Tasks,
            Spacer,
            Weather,
            Updates,
            Tray,
            RemovableVolumes,
            Clock,
        ];
        let once = migrated(input.clone());
        assert_eq!(once, input);
        assert_eq!(migrated(once.clone()), once);
    }

    #[test]
    fn migrate_inserts_tasks_but_not_optional_status() {
        use BarWidgetId::*;
        // Pre-Tasks layout with a trimmed status strip (user may omit optionals).
        let input = vec![
            Workspaces,
            Spacer,
            Battery,
            Network,
            Bluetooth,
            Volume,
            Notifications,
            Clock,
        ];
        let once = migrated(input);
        assert_eq!(migrated(once.clone()), once, "migrate must be idempotent");
        assert!(once.contains(&Tasks));
        // Optional status icons are not force-restored.
        assert!(!once.contains(&Weather));
        assert!(!once.contains(&Vpn));
        assert!(!once.contains(&Clipboard));
        assert!(!once.contains(&Tray));
        assert!(!once.contains(&RemovableVolumes));
        assert!(!once.contains(&Updates));
        // Notifications merge into the clock affordance when both are present.
        assert!(!once.contains(&Notifications));
        assert!(once.contains(&Clock));
    }

    #[test]
    fn migrate_keeps_optional_status_removed() {
        use BarWidgetId::*;
        let without_weather = vec![
            Workspaces,
            Tasks,
            Spacer,
            RemovableVolumes,
            Tray,
            Updates,
            Battery,
            Network,
            Vpn,
            Bluetooth,
            Volume,
            Clipboard,
            Clock,
        ];
        let once = migrated(without_weather.clone());
        assert_eq!(once, without_weather);
        assert_eq!(migrated(once.clone()), once);
        assert!(!once.contains(&Weather));
    }

    #[test]
    fn migrate_strips_notifications_when_clock_present() {
        use BarWidgetId::*;
        let input = vec![
            Workspaces,
            Tasks,
            Spacer,
            RemovableVolumes,
            Tray,
            Updates,
            Weather,
            Notifications,
            Clock,
        ];
        let once = migrated(input);
        assert_eq!(migrated(once.clone()), once);
        assert!(!once.contains(&Notifications));
        assert!(once.contains(&Clock));
    }

    #[test]
    fn migrate_legacy_layout_wipes_to_defaults_and_converges() {
        use BarWidgetId::*;
        let legacy = vec![
            Workspaces,
            Spacer,
            Clock,
            Spacer,
            Battery,
            Network,
            Bluetooth,
            Volume,
            Notifications,
        ];
        let once = migrated(legacy);
        assert_eq!(once, default_widgets());
        assert_eq!(migrated(once.clone()), once);
    }

    #[test]
    fn migrate_rewrites_legacy_clock_format() {
        let mut cfg = BarConfig {
            clock: ClockConfig {
                time_format: "%H:%M".into(),
                ..ClockConfig::default()
            },
            ..BarConfig::default()
        };
        migrate_bar_config(&mut cfg, false);
        assert_ne!(cfg.clock.time_format, "%H:%M");
        assert_eq!(cfg.clock.time_format, default_time_format());
        let format_after = cfg.clock.time_format.clone();
        migrate_bar_config(&mut cfg, false);
        assert_eq!(cfg.clock.time_format, format_after);
    }

    #[test]
    fn sanitize_bar_config_is_idempotent() {
        let mut cfg = BarConfig {
            length_percent: 200,
            auto_hide_delay_ms: 9_999,
            auto_hide_peek_px: 1,
            bar_fill: BarFill {
                color: "not-a-color".into(),
                gradient: vec!["#ff".into()],
                ..BarFill::default()
            },
            ..BarConfig::default()
        };
        sanitize_bar_config(&mut cfg);
        let once = cfg.clone();
        sanitize_bar_config(&mut cfg);
        assert_eq!(cfg, once);
        assert_eq!(cfg.length_percent, 100);
        assert_eq!(cfg.auto_hide_delay_ms, 5000);
        assert_eq!(cfg.auto_hide_peek_px, 2);
    }
}
