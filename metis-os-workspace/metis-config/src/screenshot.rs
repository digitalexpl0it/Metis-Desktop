use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotMode {
    #[default]
    Selection,
    Screen,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AfterCaptureAction {
    Copy,
    Save,
    CopyAndSave,
    Open,
    /// Open the Metis screenshot editor (`metis-screenshot`).
    #[default]
    Edit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotConfig {
    #[serde(default)]
    pub default_mode: ScreenshotMode,
    #[serde(default)]
    pub draw_cursor: bool,
    #[serde(default)]
    pub delay_seconds: u32,
    /// After-capture action for interactive PrtSc. Defaults to opening the editor.
    #[serde(default)]
    pub after_capture: AfterCaptureAction,
    /// Optional override for interactive PrtSc. When absent, uses `after_capture`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive_after_capture: Option<AfterCaptureAction>,
    /// After-capture action for Shift+PrtSc. Never opens the editor.
    #[serde(default = "default_instant_after_capture")]
    pub instant_after_capture: AfterCaptureAction,
    /// Prefer the output under the pointer for picker + capture (multi-monitor).
    #[serde(default = "default_true")]
    pub prefer_pointer_output: bool,
    #[serde(default = "default_save_dir")]
    pub save_dir: String,
}

fn default_save_dir() -> String {
    "~/Pictures/Metis".into()
}

fn default_true() -> bool {
    true
}

fn default_instant_after_capture() -> AfterCaptureAction {
    AfterCaptureAction::Copy
}

impl Default for ScreenshotConfig {
    fn default() -> Self {
        Self {
            default_mode: ScreenshotMode::Selection,
            draw_cursor: false,
            delay_seconds: 0,
            after_capture: AfterCaptureAction::Edit,
            interactive_after_capture: None,
            instant_after_capture: AfterCaptureAction::Copy,
            prefer_pointer_output: true,
            save_dir: default_save_dir(),
        }
    }
}

impl ScreenshotConfig {
    /// Action after interactive capture (picker). Defaults to Edit.
    pub fn interactive_action(&self) -> AfterCaptureAction {
        self.interactive_after_capture.unwrap_or(self.after_capture)
    }

    /// Action after instant full-screen capture. Never opens the editor.
    pub fn instant_action(&self) -> AfterCaptureAction {
        match self.instant_after_capture {
            AfterCaptureAction::Edit | AfterCaptureAction::Open => AfterCaptureAction::Copy,
            other => other,
        }
    }
}

pub fn screenshot_config_path() -> std::path::PathBuf {
    super::config_dir().join("screenshot.json")
}

pub fn load_screenshot_config() -> ScreenshotConfig {
    let path = screenshot_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        // Retired scroll-capture mode: map leftover configs to Selection so
        // the rest of the file still loads.
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) {
            if value.get("default_mode").and_then(|v| v.as_str()) == Some("scroll") {
                value["default_mode"] = serde_json::json!("selection");
            }
            if let Ok(cfg) = serde_json::from_value(value) {
                return sanitize_screenshot_config(cfg);
            }
        }
    }
    ScreenshotConfig::default()
}

pub fn save_default_screenshot_config() -> std::io::Result<()> {
    let path = screenshot_config_path();
    if path.exists() {
        return Ok(());
    }
    save_screenshot_config(&ScreenshotConfig::default())
}

pub fn save_screenshot_config(config: &ScreenshotConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(&sanitize_screenshot_config(config.clone()))
        .map_err(std::io::Error::other)?;
    std::fs::write(screenshot_config_path(), json)
}

fn sanitize_screenshot_config(mut cfg: ScreenshotConfig) -> ScreenshotConfig {
    cfg.delay_seconds = cfg.delay_seconds.min(30);
    if cfg.save_dir.trim().is_empty() {
        cfg.save_dir = default_save_dir();
    }
    // Older Settings builds stored Instant in `after_capture` and Interactive in
    // `interactive_after_capture`. Promote the interactive choice to the primary
    // field and keep Instant separate so Edit stays the interactive default.
    if let Some(interactive) = cfg.interactive_after_capture.take() {
        if !matches!(
            cfg.after_capture,
            AfterCaptureAction::Edit | AfterCaptureAction::Open
        ) {
            cfg.instant_after_capture = cfg.after_capture;
        }
        cfg.after_capture = interactive;
    }
    if matches!(
        cfg.instant_after_capture,
        AfterCaptureAction::Edit | AfterCaptureAction::Open
    ) {
        cfg.instant_after_capture = AfterCaptureAction::Copy;
    }
    cfg
}

pub fn expand_save_dir(path: &str) -> std::path::PathBuf {
    let trimmed = path.trim();
    if trimmed.starts_with("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return std::path::PathBuf::from(home).join(trimmed.trim_start_matches("~/"));
    }
    if trimmed == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return std::path::PathBuf::from(home);
    }
    std::path::PathBuf::from(trimmed)
}
