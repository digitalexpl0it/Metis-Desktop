//! Virtual-desktop layout for MetisNative multi-monitor RDP.
//!
//! Written to `$XDG_RUNTIME_DIR/metis/rdp-layout.json` for the FreeRDP shadow
//! subsystem and for capture blit regions.

use std::path::{Path, PathBuf};

use metis_protocol::{
    CompositorCommand, CompositorEvent, OutputInfo, RUDP_MAX_SUBSCRIBED_OUTPUTS,
    send_compositor_command,
};
use serde::{Deserialize, Serialize};

pub const LAYOUT_VERSION: u32 = 1;
pub const MAX_MONITORS: usize = RUDP_MAX_SUBSCRIBED_OUTPUTS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RdpMonitor {
    pub name: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RdpLayout {
    #[serde(default = "layout_version")]
    pub version: u32,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Global origin subtracted when blitting into the atlas (min left/top).
    #[serde(default)]
    pub origin_x: i32,
    #[serde(default)]
    pub origin_y: i32,
    #[serde(default)]
    pub monitors: Vec<RdpMonitor>,
}

fn layout_version() -> u32 {
    LAYOUT_VERSION
}

impl Default for RdpLayout {
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            atlas_width: 1920,
            atlas_height: 1080,
            origin_x: 0,
            origin_y: 0,
            monitors: vec![RdpMonitor {
                name: "default".into(),
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
                primary: true,
            }],
        }
    }
}

impl RdpMonitor {
    pub fn width(&self) -> u32 {
        self.right.saturating_sub(self.left).max(0) as u32
    }

    pub fn height(&self) -> u32 {
        self.bottom.saturating_sub(self.top).max(0) as u32
    }

    /// Atlas-relative blit origin.
    pub fn atlas_x(&self, origin_x: i32) -> i32 {
        self.left - origin_x
    }

    pub fn atlas_y(&self, origin_y: i32) -> i32 {
        self.top - origin_y
    }
}

pub fn layout_path(runtime_metis: &Path) -> PathBuf {
    runtime_metis.join("rdp-layout.json")
}

/// Query compositor outputs and build a sanitized multi-monitor layout.
pub fn fetch_layout_from_compositor() -> RdpLayout {
    match send_compositor_command(&CompositorCommand::ListOutputs) {
        Ok(CompositorEvent::OutputList { outputs }) => layout_from_outputs(&outputs),
        Ok(_) | Err(_) => {
            tracing::warn!("rdp host: ListOutputs failed — using single-monitor default");
            RdpLayout::default()
        }
    }
}

pub fn layout_from_outputs(outputs: &[OutputInfo]) -> RdpLayout {
    let mut enabled: Vec<&OutputInfo> = outputs
        .iter()
        .filter(|o| o.enabled && !o.mirrored && o.rect.width > 0 && o.rect.height > 0)
        .collect();
    enabled.sort_by(|a, b| {
        // Primary first, then left-to-right / top-to-bottom.
        b.primary
            .cmp(&a.primary)
            .then(a.rect.x.cmp(&b.rect.x))
            .then(a.rect.y.cmp(&b.rect.y))
    });
    if enabled.len() > MAX_MONITORS {
        enabled.truncate(MAX_MONITORS);
    }
    if enabled.is_empty() {
        return RdpLayout::default();
    }

    let monitors: Vec<RdpMonitor> = enabled
        .iter()
        .map(|o| RdpMonitor {
            name: o.name.clone(),
            left: o.rect.x,
            top: o.rect.y,
            right: o.rect.x.saturating_add(o.rect.width),
            bottom: o.rect.y.saturating_add(o.rect.height),
            primary: o.primary,
        })
        .collect();

    sanitize_layout(RdpLayout {
        version: LAYOUT_VERSION,
        atlas_width: 0,
        atlas_height: 0,
        origin_x: 0,
        origin_y: 0,
        monitors,
    })
}

pub fn sanitize_layout(mut layout: RdpLayout) -> RdpLayout {
    layout.version = LAYOUT_VERSION;
    layout.monitors.retain(|m| m.width() > 0 && m.height() > 0);
    if layout.monitors.len() > MAX_MONITORS {
        layout.monitors.truncate(MAX_MONITORS);
    }
    if layout.monitors.is_empty() {
        return RdpLayout::default();
    }
    if !layout.monitors.iter().any(|m| m.primary) {
        layout.monitors[0].primary = true;
    }
    let origin_x = layout.monitors.iter().map(|m| m.left).min().unwrap_or(0);
    let origin_y = layout.monitors.iter().map(|m| m.top).min().unwrap_or(0);
    let max_right = layout
        .monitors
        .iter()
        .map(|m| m.right)
        .max()
        .unwrap_or(1920);
    let max_bottom = layout
        .monitors
        .iter()
        .map(|m| m.bottom)
        .max()
        .unwrap_or(1080);
    layout.origin_x = origin_x;
    layout.origin_y = origin_y;
    layout.atlas_width = (max_right - origin_x).max(1) as u32;
    layout.atlas_height = (max_bottom - origin_y).max(1) as u32;
    layout
}

pub fn save_layout(path: &Path, layout: &RdpLayout) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let clean = sanitize_layout(layout.clone());
    let json = serde_json::to_vec_pretty(&clean).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Match a ScreenCast stream (position/size) to a layout monitor for atlas blit.
pub fn match_monitor_for_stream(
    layout: &RdpLayout,
    pos_x: i32,
    pos_y: i32,
    width: u32,
    height: u32,
    stream_index: usize,
) -> Option<&RdpMonitor> {
    // Prefer exact position match from portal Stream.position.
    if let Some(m) = layout
        .monitors
        .iter()
        .find(|m| m.left == pos_x && m.top == pos_y)
    {
        return Some(m);
    }
    // Size + rough position.
    if let Some(m) = layout.monitors.iter().find(|m| {
        m.width() == width
            && m.height() == height
            && (m.left - pos_x).unsigned_abs() < 8
            && (m.top - pos_y).unsigned_abs() < 8
    }) {
        return Some(m);
    }
    layout.monitors.get(stream_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metis_protocol::MonitorRect;

    fn out(name: &str, x: i32, y: i32, w: i32, h: i32, primary: bool) -> OutputInfo {
        OutputInfo {
            name: name.into(),
            primary,
            rect: MonitorRect {
                x,
                y,
                width: w,
                height: h,
            },
            scale: 1.0,
            enabled: true,
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            mirrored: false,
            mirror_source: false,
            vrr_available: false,
            vrr_active: false,
            hdr_available: false,
            hdr_active: false,
        }
    }

    #[test]
    fn dual_monitor_bbox() {
        let layout = layout_from_outputs(&[
            out("DP-1", 0, 0, 1920, 1080, true),
            out("HDMI-1", 1920, 0, 1920, 1080, false),
        ]);
        assert_eq!(layout.monitors.len(), 2);
        assert_eq!(layout.atlas_width, 3840);
        assert_eq!(layout.atlas_height, 1080);
        assert_eq!(layout.origin_x, 0);
        assert!(layout.monitors[0].primary);
    }

    #[test]
    fn negative_origin() {
        let layout = layout_from_outputs(&[
            out("left", -1920, 0, 1920, 1080, false),
            out("main", 0, 0, 1920, 1080, true),
        ]);
        assert_eq!(layout.origin_x, -1920);
        assert_eq!(layout.atlas_width, 3840);
        let left = layout
            .monitors
            .iter()
            .find(|m| m.name == "left")
            .expect("left");
        let main = layout
            .monitors
            .iter()
            .find(|m| m.name == "main")
            .expect("main");
        assert_eq!(left.atlas_x(layout.origin_x), 0);
        assert_eq!(main.atlas_x(layout.origin_x), 1920);
    }

    #[test]
    fn sanitize_caps_at_four() {
        let outs: Vec<_> = (0..6)
            .map(|i| out(&format!("o{i}"), i * 100, 0, 100, 100, i == 0))
            .collect();
        let layout = layout_from_outputs(&outs);
        assert_eq!(layout.monitors.len(), MAX_MONITORS);
    }

    #[test]
    fn match_by_position() {
        let layout = layout_from_outputs(&[
            out("a", 0, 0, 800, 600, true),
            out("b", 800, 0, 800, 600, false),
        ]);
        let m = match_monitor_for_stream(&layout, 800, 0, 800, 600, 0).expect("match");
        assert_eq!(m.name, "b");
    }

    #[test]
    fn sanitize_idempotent() {
        let once = sanitize_layout(layout_from_outputs(&[out("a", 0, 0, 100, 100, true)]));
        let twice = sanitize_layout(once.clone());
        assert_eq!(once, twice);
    }
}
