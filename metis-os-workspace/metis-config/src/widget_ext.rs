//! Desktop widget extension packs (Phase 14 §E).
//!
//! Declarative JSON widgets live under:
//! - `~/.local/share/metis/widgets/<id>/`
//! - `/usr/share/metis/widgets/<id>/` (and `/usr/local/share/...`)
//!
//! Each pack has `manifest.json` + `widget.json`. No scripts or `.so` loads.
//!
//! **Host action hardening (v1):** `open_uri` is http(s) only; `launch` is
//! desktop-id or a single PATH basename (no argv / absolute paths); action
//! fields are not settings-interpolated; layout trees are size-capped.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Current host API level understood by Metis.
pub const WIDGET_EXT_API: u32 = 1;

/// Reject oversized `widget.json` files before parse.
pub const WIDGET_EXT_MAX_JSON_BYTES: u64 = 256 * 1024;
/// Max nesting depth for layout nodes.
pub const WIDGET_EXT_MAX_DEPTH: usize = 12;
/// Max total nodes in one layout tree.
pub const WIDGET_EXT_MAX_NODES: usize = 256;
/// Max characters for label / button / copy / URI strings after validation.
pub const WIDGET_EXT_MAX_STRING: usize = 2048;
/// Max clipboard payload from `copy_text`.
pub const WIDGET_EXT_MAX_COPY: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetExtManifest {
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_api")]
    pub api: u32,
    /// `[width, height]` default card size.
    #[serde(default = "default_size_pair")]
    pub default_size: [u32; 2],
    #[serde(default)]
    pub min_size: Option<[u32; 2]>,
    #[serde(default)]
    pub settings_schema: Vec<WidgetExtSetting>,
    /// Optional out-of-process helper (Phase 14 §E.2). Basename under the pack
    /// root only — spawned argv-only with stdout JSON `{ "key": "value", … }`.
    #[serde(default)]
    pub helper: Option<WidgetExtHelper>,
}

/// Out-of-process helper declared by a widget pack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetExtHelper {
    /// File name inside the pack directory (no `/`, no `..`).
    pub exec: String,
    /// How often the host re-runs the helper (clamped 2–120 s).
    #[serde(default = "default_helper_poll")]
    pub poll_seconds: u32,
}

fn default_helper_poll() -> u32 {
    5
}

/// Max stdout bytes accepted from a helper.
pub const WIDGET_EXT_HELPER_MAX_STDOUT: usize = 8 * 1024;
/// Hard timeout for one helper run.
pub const WIDGET_EXT_HELPER_TIMEOUT_SECS: u64 = 3;

/// Resolve and validate `helper.exec` to an absolute path under `pack_root`.
pub fn resolve_helper_exec(pack_root: &Path, helper: &WidgetExtHelper) -> Option<PathBuf> {
    let name = helper.exec.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    if name.starts_with('.') {
        return None;
    }
    let path = pack_root.join(name);
    let root = pack_root.canonicalize().ok()?;
    let canon = path.canonicalize().ok()?;
    if !canon.starts_with(&root) {
        return None;
    }
    if !canon.is_file() {
        return None;
    }
    Some(canon)
}

/// Run a pack helper (argv-only). Expects stdout JSON object of string/number/bool
/// values flattened to string map. Times out and kills the child on hang.
pub fn run_helper_snapshot(
    exec: &Path,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    let mut child = Command::new(exec)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env(
            "LANG",
            std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()),
        )
        .spawn()
        .map_err(|e| format!("spawn helper: {e}"))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "helper missing stdout".to_string())?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            if buf.len() >= WIDGET_EXT_HELPER_MAX_STDOUT {
                break;
            }
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let room = WIDGET_EXT_HELPER_MAX_STDOUT.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                    if n > room {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(buf);
    });

    let timeout = Duration::from_secs(WIDGET_EXT_HELPER_TIMEOUT_SECS);
    let buf = match rx.recv_timeout(timeout) {
        Ok(b) => b,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("helper timed out".into());
        }
    };
    let status = child.wait().map_err(|e| format!("wait helper: {e}"))?;
    if !status.success() {
        return Err(format!("helper exit {status}"));
    }
    let text = String::from_utf8_lossy(&buf);
    let value: serde_json::Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("helper JSON: {e}"))?;
    let mut out = std::collections::BTreeMap::new();
    let Some(obj) = value.as_object() else {
        return Err("helper JSON must be an object".into());
    };
    for (k, v) in obj {
        if !is_safe_helper_key(k) {
            continue;
        }
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => String::new(),
            _ => continue,
        };
        if s.len() > 512 {
            continue;
        }
        out.insert(k.clone(), s);
    }
    Ok(out)
}

fn is_safe_helper_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn default_version() -> String {
    "1.0.0".into()
}

fn default_api() -> u32 {
    WIDGET_EXT_API
}

fn default_size_pair() -> [u32; 2] {
    [320, 200]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WidgetExtSettingType {
    #[default]
    String,
    Bool,
    Number,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetExtSetting {
    pub key: String,
    #[serde(rename = "type", default)]
    pub setting_type: WidgetExtSettingType,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub default: serde_json::Value,
}

/// Discovered pack ready for Settings / host.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredWidgetExt {
    pub manifest: WidgetExtManifest,
    /// Directory containing manifest.json + widget.json.
    pub root: PathBuf,
}

/// Result of a fail-closed pack scan (Phase 18 C).
#[derive(Debug, Clone, Default)]
pub struct WidgetExtDiscoverResult {
    pub packs: Vec<DiscoveredWidgetExt>,
    /// `(pack_dir, reason)` for each directory that failed validation.
    pub skipped: Vec<(PathBuf, String)>,
}

/// Validate reverse-DNS-ish extension ids (`com.metis.example.quicklinks`).
pub fn is_valid_extension_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    if id.starts_with('.') || id.ends_with('.') || id.contains("..") {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
        && id.contains('.')
}

/// Directories searched for widget packs (user first, then system).
pub fn widget_ext_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = directories::ProjectDirs::from("com", "metis", "metis") {
        dirs.push(data.data_local_dir().join("widgets"));
    } else if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/metis/widgets"));
    }
    for base in ["/usr/local/share/metis/widgets", "/usr/share/metis/widgets"] {
        dirs.push(PathBuf::from(base));
    }
    dirs
}

/// Scan search dirs; later duplicates of the same id are ignored (user wins).
pub fn discover_widget_extensions() -> Vec<DiscoveredWidgetExt> {
    discover_widget_extensions_detailed().packs
}

/// Like [`discover_widget_extensions`], but also reports skipped invalid packs.
pub fn discover_widget_extensions_detailed() -> WidgetExtDiscoverResult {
    let mut packs = Vec::new();
    let mut skipped = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in widget_ext_search_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            match load_widget_extension(&path) {
                Ok(ext) => {
                    if seen.insert(ext.manifest.id.clone()) {
                        packs.push(ext);
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        %err,
                        "skipping invalid widget extension pack"
                    );
                    skipped.push((path, err));
                }
            }
        }
    }
    packs.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    WidgetExtDiscoverResult { packs, skipped }
}

pub fn load_widget_extension(root: &Path) -> Result<DiscoveredWidgetExt, String> {
    let manifest_path = root.join("manifest.json");
    let widget_path = root.join("widget.json");
    if !manifest_path.is_file() {
        return Err("missing manifest.json".into());
    }
    if !widget_path.is_file() {
        return Err("missing widget.json".into());
    }
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
    let manifest: WidgetExtManifest =
        serde_json::from_str(&text).map_err(|e| format!("manifest.json: {e}"))?;
    if !is_valid_extension_id(&manifest.id) {
        return Err(format!("invalid extension id {:?}", manifest.id));
    }
    if let Some(dir_name) = root.file_name().and_then(|s| s.to_str())
        && dir_name != manifest.id
    {
        return Err(format!(
            "folder name {dir_name:?} must match manifest id {:?}",
            manifest.id
        ));
    }
    if manifest.api != WIDGET_EXT_API {
        return Err(format!(
            "unsupported api {} (need {WIDGET_EXT_API})",
            manifest.api
        ));
    }
    // Reject path traversal in settings keys.
    for setting in &manifest.settings_schema {
        if setting.key.is_empty()
            || setting.key.contains('/')
            || setting.key.contains('\\')
            || setting.key.contains("..")
        {
            return Err(format!("invalid settings key {:?}", setting.key));
        }
    }
    // Fail closed: layout must parse and pass action/size validators at discovery.
    load_widget_layout(root).map_err(|e| format!("widget.json: {e}"))?;
    if let Some(helper) = &manifest.helper
        && resolve_helper_exec(root, helper).is_none()
    {
        return Err(format!(
            "helper.exec {:?} missing or escapes pack root",
            helper.exec
        ));
    }
    Ok(DiscoveredWidgetExt {
        manifest,
        root: root.to_path_buf(),
    })
}

pub fn find_widget_extension(id: &str) -> Option<DiscoveredWidgetExt> {
    if !is_valid_extension_id(id) {
        return None;
    }
    discover_widget_extensions()
        .into_iter()
        .find(|e| e.manifest.id == id)
}

pub fn load_widget_layout(root: &Path) -> Result<WidgetExtNode, String> {
    let path = root.join("widget.json");
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() > WIDGET_EXT_MAX_JSON_BYTES {
        return Err(format!(
            "widget.json too large ({} bytes; max {WIDGET_EXT_MAX_JSON_BYTES})",
            meta.len()
        ));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let node: WidgetExtNode =
        serde_json::from_str(&text).map_err(|e| format!("widget.json: {e}"))?;
    validate_widget_layout(&node)?;
    Ok(node)
}

/// Walk the layout tree: depth/node caps + harden every action at load time.
pub fn validate_widget_layout(root: &WidgetExtNode) -> Result<(), String> {
    let mut nodes = 0usize;
    validate_node(root, 0, &mut nodes)
}

fn validate_node(node: &WidgetExtNode, depth: usize, nodes: &mut usize) -> Result<(), String> {
    if depth > WIDGET_EXT_MAX_DEPTH {
        return Err(format!(
            "widget.json nesting exceeds max depth {WIDGET_EXT_MAX_DEPTH}"
        ));
    }
    *nodes += 1;
    if *nodes > WIDGET_EXT_MAX_NODES {
        return Err(format!(
            "widget.json has more than {WIDGET_EXT_MAX_NODES} nodes"
        ));
    }
    match node {
        WidgetExtNode::Column { children, .. }
        | WidgetExtNode::Row { children, .. }
        | WidgetExtNode::List { children, .. } => {
            for child in children {
                validate_node(child, depth + 1, nodes)?;
            }
            Ok(())
        }
        WidgetExtNode::Label { text, .. } => {
            if text.len() > WIDGET_EXT_MAX_STRING {
                return Err("label text too long".into());
            }
            Ok(())
        }
        WidgetExtNode::Icon { name, .. } => {
            if !is_safe_icon_name(name) {
                return Err(format!("invalid icon name {name:?}"));
            }
            Ok(())
        }
        WidgetExtNode::Button { label, on_click } => {
            if label.len() > WIDGET_EXT_MAX_STRING {
                return Err("button label too long".into());
            }
            validate_action(on_click)
        }
        WidgetExtNode::Separator => Ok(()),
    }
}

/// Validate a button action (also used by the host before running).
pub fn validate_action(action: &WidgetExtAction) -> Result<(), String> {
    match action {
        WidgetExtAction::OpenUri { uri } => {
            if !is_safe_open_uri(uri) {
                return Err(format!(
                    "open_uri rejected (http/https only, max {WIDGET_EXT_MAX_STRING} chars): {uri:?}"
                ));
            }
            Ok(())
        }
        WidgetExtAction::Launch { id, exec } => {
            let id = id.trim();
            let exec = exec.trim();
            if !id.is_empty() {
                if !is_safe_launch_id(id) {
                    return Err(format!("launch id rejected: {id:?}"));
                }
                return Ok(());
            }
            if !exec.is_empty() {
                if !is_safe_launch_exec(exec) {
                    return Err(format!(
                        "launch exec rejected (single PATH basename only): {exec:?}"
                    ));
                }
                return Ok(());
            }
            Err("launch action missing id and exec".into())
        }
        WidgetExtAction::CopyText { text } => {
            if text.len() > WIDGET_EXT_MAX_COPY {
                return Err(format!(
                    "copy_text exceeds max {WIDGET_EXT_MAX_COPY} characters"
                ));
            }
            Ok(())
        }
    }
}

/// `http://` or `https://` only — no `file:`, custom schemes, bare paths, or userinfo.
pub fn is_safe_open_uri(uri: &str) -> bool {
    let uri = uri.trim();
    if uri.is_empty() || uri.len() > WIDGET_EXT_MAX_STRING {
        return false;
    }
    if uri.contains(|c: char| c.is_control()) {
        return false;
    }
    let Some((scheme, rest)) = uri.split_once("://") else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    // Authority ends at path / query / fragment.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = authority
        .rsplit_once(':')
        .map(|(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                authority // IPv6 without brackets mishandled — fall through
            }
        })
        .unwrap_or(authority);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    !host.is_empty()
        && host
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Desktop / app id for `launch` (e.g. `metis-settings`, `org.gnome.Calculator`).
pub fn is_safe_launch_id(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || id.len() > 128 {
        return false;
    }
    if id.starts_with('.') || id.ends_with('.') || id.contains("..") || id.contains('/') {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Single executable basename on `$PATH` — no paths, argv, or shell metacharacters.
pub fn is_safe_launch_exec(exec: &str) -> bool {
    let exec = exec.trim();
    if exec.is_empty() || exec.len() > 64 {
        return false;
    }
    if exec.contains('/') || exec.contains('\\') || exec.contains('\0') {
        return false;
    }
    if exec.contains(char::is_whitespace) {
        return false;
    }
    // No shell / argv injection surface.
    if exec.contains(|c: char| {
        matches!(
            c,
            ';' | '|' | '&' | '$' | '`' | '(' | ')' | '<' | '>' | '\'' | '"' | '\\' | '\n' | '\r'
        )
    }) {
        return false;
    }
    if !exec
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+')
    {
        return false;
    }
    // Prefer desktop `id`; when `exec` is used, block obvious interpreters/shells.
    !LAUNCH_EXEC_DENYLIST
        .iter()
        .any(|blocked| exec.eq_ignore_ascii_case(blocked))
}

/// Basenames rejected for extension `launch.exec` (use a `.desktop` id instead).
const LAUNCH_EXEC_DENYLIST: &[&str] = &[
    "sh",
    "bash",
    "dash",
    "zsh",
    "fish",
    "csh",
    "tcsh",
    "ksh",
    "busybox",
    "env",
    "sudo",
    "doas",
    "pkexec",
    "python",
    "python2",
    "python3",
    "perl",
    "ruby",
    "node",
    "nodejs",
    "lua",
    "php",
    "powershell",
    "pwsh",
    "cmd",
    "cmd.exe",
];

/// Icon theme names only (no paths).
pub fn is_safe_icon_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' || c == '+')
}

/// Default settings map from schema.
pub fn default_extension_settings(
    schema: &[WidgetExtSetting],
) -> serde_json::Map<String, serde_json::Value> {
    let mut map = serde_json::Map::new();
    for s in schema {
        let val = if s.default.is_null() {
            match s.setting_type {
                WidgetExtSettingType::String => serde_json::Value::String(String::new()),
                WidgetExtSettingType::Bool => serde_json::Value::Bool(false),
                WidgetExtSettingType::Number => serde_json::json!(0),
            }
        } else {
            s.default.clone()
        };
        map.insert(s.key.clone(), val);
    }
    map
}

/// Declarative layout nodes (widget.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WidgetExtNode {
    Column {
        #[serde(default)]
        spacing: i32,
        #[serde(default)]
        children: Vec<WidgetExtNode>,
    },
    Row {
        #[serde(default)]
        spacing: i32,
        #[serde(default)]
        children: Vec<WidgetExtNode>,
    },
    Label {
        #[serde(default)]
        text: String,
        #[serde(default)]
        style: WidgetExtLabelStyle,
    },
    Icon {
        name: String,
        #[serde(default = "default_icon_px")]
        pixel_size: i32,
    },
    Button {
        #[serde(default)]
        label: String,
        on_click: WidgetExtAction,
    },
    Separator,
    List {
        #[serde(default)]
        spacing: i32,
        #[serde(default)]
        children: Vec<WidgetExtNode>,
    },
}

fn default_icon_px() -> i32 {
    24
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetExtLabelStyle {
    #[default]
    Body,
    Title,
    Muted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WidgetExtAction {
    OpenUri {
        uri: String,
    },
    Launch {
        /// Desktop app id (e.g. `org.gnome.Calculator`) or executable name.
        #[serde(default)]
        id: String,
        #[serde(default)]
        exec: String,
    },
    CopyText {
        text: String,
    },
}

/// Replace `{settings.key}` placeholders using extension settings.
pub fn interpolate_settings(
    template: &str,
    settings: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let mut out = template.to_string();
    for (key, val) in settings {
        let needle = format!("{{settings.{key}}}");
        let repl = match val {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };
        out = out.replace(&needle, &repl);
    }
    out
}

/// Live host values for `{host.*}` tokens (Phase 14 §E.2 slice).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostBindValues {
    pub time: String,
    pub date: String,
    pub weather_temp: String,
    pub weather_unit: String,
    pub weather_summary: String,
    pub sys_cpu: String,
    pub sys_mem: String,
    pub sys_disk: String,
    /// Flat map from out-of-process helper stdout JSON (`{helper.key}`).
    pub helper: std::collections::BTreeMap<String, String>,
}

/// True when `template` references any `{host.…}` or `{helper.…}` token.
pub fn template_needs_host(template: &str) -> bool {
    template.contains("{host.") || template.contains("{helper.")
}

/// Settings first, then host binds, then helper keys. Unknown tokens → empty.
pub fn interpolate_template(
    template: &str,
    settings: &serde_json::Map<String, serde_json::Value>,
    host: Option<&HostBindValues>,
) -> String {
    let mut out = interpolate_settings(template, settings);
    if !template_needs_host(&out) && !template_needs_host(template) {
        return out;
    }
    let empty = HostBindValues::default();
    let host = host.unwrap_or(&empty);
    for (key, val) in [
        ("{host.time}", host.time.as_str()),
        ("{host.date}", host.date.as_str()),
        ("{host.weather.temp}", host.weather_temp.as_str()),
        ("{host.weather.unit}", host.weather_unit.as_str()),
        ("{host.weather.summary}", host.weather_summary.as_str()),
        ("{host.sys.cpu}", host.sys_cpu.as_str()),
        ("{host.sys.mem}", host.sys_mem.as_str()),
        ("{host.sys.disk}", host.sys_disk.as_str()),
    ] {
        out = out.replace(key, val);
    }
    // Replace `{helper.<key>}` from the helper map (unknown → empty).
    while let Some(start) = out.find("{helper.") {
        let rest = &out[start + "{helper.".len()..];
        let Some(end_rel) = rest.find('}') else {
            break;
        };
        let key = &rest[..end_rel];
        let repl = host.helper.get(key).map(String::as_str).unwrap_or("");
        let needle = format!("{{helper.{key}}}");
        out = out.replacen(&needle, repl, 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_extension_ids() {
        assert!(is_valid_extension_id("com.metis.example.quicklinks"));
        assert!(!is_valid_extension_id("BadId"));
        assert!(!is_valid_extension_id("no-dots"));
        assert!(!is_valid_extension_id("../etc"));
        assert!(!is_valid_extension_id(""));
    }

    #[test]
    fn hardens_open_uri() {
        assert!(is_safe_open_uri("https://example.com/path"));
        assert!(is_safe_open_uri("http://example.com"));
        assert!(is_safe_open_uri("https://example.com:8443/a"));
        assert!(!is_safe_open_uri("file:///etc/passwd"));
        assert!(!is_safe_open_uri("/etc/passwd"));
        assert!(!is_safe_open_uri("javascript:alert(1)"));
        assert!(!is_safe_open_uri("https://user:pass@evil.com/"));
        assert!(!is_safe_open_uri("https://user@evil.com/"));
        assert!(!is_safe_open_uri("ftp://example.com"));
        assert!(!is_safe_open_uri(""));
    }

    #[test]
    fn hardens_launch_targets() {
        assert!(is_safe_launch_id("metis-settings"));
        assert!(is_safe_launch_id("org.gnome.Calculator"));
        assert!(!is_safe_launch_id("../bin"));
        assert!(!is_safe_launch_id("a/b"));
        assert!(is_safe_launch_exec("metis-settings"));
        assert!(is_safe_launch_exec("firefox"));
        assert!(!is_safe_launch_exec("/bin/sh"));
        assert!(!is_safe_launch_exec("rm -rf /"));
        assert!(!is_safe_launch_exec("sh"));
        assert!(!is_safe_launch_exec("bash"));
        assert!(!is_safe_launch_exec("python3"));
        assert!(!is_safe_launch_exec("bash -c id"));
    }

    #[test]
    fn rejects_unsafe_actions_in_layout() {
        let bad = WidgetExtNode::Button {
            label: "x".into(),
            on_click: WidgetExtAction::OpenUri {
                uri: "file:///tmp".into(),
            },
        };
        assert!(validate_widget_layout(&bad).is_err());

        let good = WidgetExtNode::Button {
            label: "x".into(),
            on_click: WidgetExtAction::OpenUri {
                uri: "https://example.com".into(),
            },
        };
        assert!(validate_widget_layout(&good).is_ok());
    }

    #[test]
    fn interpolates_helper_tokens() {
        let mut helper = std::collections::BTreeMap::new();
        helper.insert("uname".into(), "Linux".into());
        let host = HostBindValues {
            time: "12:00:00".into(),
            helper,
            ..HostBindValues::default()
        };
        let settings = serde_json::Map::new();
        assert_eq!(
            interpolate_template("os={helper.uname} t={host.time}", &settings, Some(&host)),
            "os=Linux t=12:00:00"
        );
        assert!(template_needs_host("{helper.uname}"));
    }

    #[test]
    fn loads_pack_from_temp() {
        let pack = std::env::temp_dir()
            .join(format!("metis-wext-{}", std::process::id()))
            .join("com.metis.test.pack");
        let _ = std::fs::remove_dir_all(pack.parent().unwrap());
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("manifest.json"),
            r#"{
              "id": "com.metis.test.pack",
              "name": "Test",
              "version": "1.0.0",
              "api": 1,
              "default_size": [300, 180],
              "settings_schema": [
                { "key": "title", "type": "string", "label": "Title", "default": "Hi" }
              ]
            }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("widget.json"),
            r#"{
              "type": "column",
              "spacing": 8,
              "children": [
                { "type": "label", "text": "{settings.title}", "style": "title" },
                { "type": "button", "label": "Open", "on_click": { "action": "open_uri", "uri": "https://example.com" } }
              ]
            }"#,
        )
        .unwrap();
        let ext = load_widget_extension(&pack).expect("load");
        assert_eq!(ext.manifest.name, "Test");
        let layout = load_widget_layout(&pack).expect("layout");
        match layout {
            WidgetExtNode::Column { children, .. } => assert_eq!(children.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
        let settings = default_extension_settings(&ext.manifest.settings_schema);
        assert_eq!(
            interpolate_settings("X {settings.title} Y", &settings),
            "X Hi Y"
        );
        let _ = std::fs::remove_dir_all(pack.parent().unwrap());
    }

    #[test]
    fn rejects_manifest_unknown_fields() {
        let err = serde_json::from_str::<WidgetExtManifest>(
            r#"{
              "id": "com.metis.test.pack",
              "name": "Test",
              "api": 1,
              "evil": true
            }"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("unknown field") || err.contains("evil"),
            "{err}"
        );
    }

    #[test]
    fn rejects_layout_unknown_fields() {
        let cases = [
            r#"{"type":"label","text":"x","nope":1}"#,
            r#"{"type":"button","label":"x","on_click":{"action":"open_uri","uri":"https://a.com","extra":1}}"#,
            r#"{"type":"weird"}"#,
        ];
        for json in cases {
            assert!(
                serde_json::from_str::<WidgetExtNode>(json).is_err(),
                "expected reject for {json}"
            );
        }
    }

    #[test]
    fn rejects_pack_with_bad_or_missing_layout() {
        let base = std::env::temp_dir().join(format!("metis-wext-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let pack = base.join("com.metis.test.bad");
        std::fs::create_dir_all(&pack).unwrap();
        let manifest = r#"{
          "id": "com.metis.test.bad",
          "name": "Bad",
          "api": 1
        }"#;
        std::fs::write(pack.join("manifest.json"), manifest).unwrap();
        // Missing widget.json
        assert!(load_widget_extension(&pack).is_err());
        // Invalid layout (unsafe URI)
        std::fs::write(
            pack.join("widget.json"),
            r#"{"type":"button","label":"x","on_click":{"action":"open_uri","uri":"file:///etc/passwd"}}"#,
        )
        .unwrap();
        assert!(load_widget_extension(&pack).is_err());
        // Missing helper binary when declared
        std::fs::write(
            pack.join("manifest.json"),
            r#"{
              "id": "com.metis.test.bad",
              "name": "Bad",
              "api": 1,
              "helper": { "exec": "missing-helper", "poll_seconds": 5 }
            }"#,
        )
        .unwrap();
        std::fs::write(pack.join("widget.json"), r#"{"type":"label","text":"ok"}"#).unwrap();
        let err = load_widget_extension(&pack).unwrap_err();
        assert!(err.contains("helper"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn accepts_example_pack_json() {
        let quicklinks =
            include_str!("../../assets/widgets/com.metis.example.quicklinks/manifest.json");
        let quick_layout =
            include_str!("../../assets/widgets/com.metis.example.quicklinks/widget.json");
        let manifest: WidgetExtManifest = serde_json::from_str(quicklinks).expect("ql manifest");
        assert_eq!(manifest.id, "com.metis.example.quicklinks");
        let node: WidgetExtNode = serde_json::from_str(quick_layout).expect("ql layout");
        assert!(validate_widget_layout(&node).is_ok());

        let helper_m =
            include_str!("../../assets/widgets/com.metis.example.helperstatus/manifest.json");
        let helper_l =
            include_str!("../../assets/widgets/com.metis.example.helperstatus/widget.json");
        let hm: WidgetExtManifest = serde_json::from_str(helper_m).expect("hs manifest");
        assert!(hm.helper.is_some());
        let hn: WidgetExtNode = serde_json::from_str(helper_l).expect("hs layout");
        assert!(validate_widget_layout(&hn).is_ok());
    }
}
