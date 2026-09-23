//! Shared live thumbnails for Task View (window cards + workspace shelf).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::prelude::*;
use metis_protocol::{CompositorCommand, CompositorEvent, WindowInfo};

#[derive(Clone)]
pub struct ThumbSet {
    pub textures: HashMap<u32, gdk::Texture>,
}

#[derive(Clone)]
pub struct WorkspaceThumbSet {
    pub textures: HashMap<u32, gdk::Texture>,
}

/// Ask the compositor to refresh window thumbs, wait briefly for PNGs, load them.
pub fn load_window_thumbs(windows: &[WindowInfo]) -> Option<ThumbSet> {
    let ids: Vec<u32> = windows.iter().map(|w| w.id).collect();
    if ids.is_empty() {
        return None;
    }

    let paths: Vec<PathBuf> = ids.iter().map(|id| thumb_path(*id)).collect();
    for path in &paths {
        let _ = std::fs::remove_file(path);
    }

    let _ = metis_protocol::send_compositor_command(&CompositorCommand::CaptureWindowThumbs {
        ids: ids.clone(),
    });

    wait_for_files(&paths, Duration::from_millis(450));

    if ids.iter().any(|id| !thumb_path(*id).is_file())
        && let Ok(CompositorEvent::WindowThumbs { .. }) =
            metis_protocol::send_compositor_command(&CompositorCommand::CaptureWindowThumbs {
                ids: ids.clone(),
            })
    {
        wait_for_files(&paths, Duration::from_millis(250));
    }

    let mut textures = HashMap::new();
    for id in ids {
        if let Some(tex) = load_png_texture(&thumb_path(id)) {
            textures.insert(id, tex);
        }
    }
    if textures.is_empty() {
        None
    } else {
        Some(ThumbSet { textures })
    }
}

/// Live mini-desktop PNGs for the Task View shelf.
pub fn load_workspace_thumbs(output: &str, workspaces: &[u32]) -> Option<WorkspaceThumbSet> {
    if output.is_empty() || workspaces.is_empty() {
        return None;
    }

    let paths: Vec<PathBuf> = workspaces
        .iter()
        .map(|ws| workspace_thumb_path(output, *ws))
        .collect();
    // Bust stale cache so we wait for the compositor's fresh GL write instead of
    // returning yesterday's PNGs (which made the shelf need a second Super+Tab).
    for path in &paths {
        let _ = std::fs::remove_file(path);
    }

    let _ = metis_protocol::send_compositor_command(&CompositorCommand::CaptureWorkspaceThumbs {
        output: output.to_string(),
        workspaces: workspaces.to_vec(),
    });

    wait_for_files(&paths, Duration::from_millis(500));

    if paths.iter().any(|p| !p.is_file())
        && let Ok(CompositorEvent::WorkspaceThumbs { .. }) =
            metis_protocol::send_compositor_command(&CompositorCommand::CaptureWorkspaceThumbs {
                output: output.to_string(),
                workspaces: workspaces.to_vec(),
            })
    {
        wait_for_files(&paths, Duration::from_millis(300));
    }

    let mut textures = HashMap::new();
    for &ws in workspaces {
        if let Some(tex) = load_png_texture(&workspace_thumb_path(output, ws)) {
            textures.insert(ws, tex);
        }
    }
    if textures.is_empty() {
        None
    } else {
        Some(WorkspaceThumbSet { textures })
    }
}

pub fn thumb_path(id: u32) -> PathBuf {
    metis_protocol::runtime_dir()
        .join("thumbs")
        .join(format!("{id}.png"))
}

pub fn workspace_thumb_path(output: &str, workspace: u32) -> PathBuf {
    let safe: String = output
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    metis_protocol::runtime_dir()
        .join("thumbs")
        .join(format!("ws-{safe}-{workspace}.png"))
}

fn wait_for_files(paths: &[PathBuf], budget: Duration) {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if paths.iter().all(|p| p.is_file()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn load_png_texture(path: &Path) -> Option<gdk::Texture> {
    if !path.is_file() {
        return None;
    }
    let file = gio::File::for_path(path);
    gdk::Texture::from_file(&file).ok()
}

/// Prefer a live thumb Picture; otherwise an app-icon Image.
pub fn thumb_or_icon_widget(
    thumbs: Option<&ThumbSet>,
    w: &WindowInfo,
    icon_px: i32,
) -> gtk::Widget {
    if let Some(tex) = thumbs.and_then(|t| t.textures.get(&w.id)) {
        let pic = gtk::Picture::for_paintable(tex);
        pic.set_content_fit(gtk::ContentFit::Contain);
        pic.set_can_shrink(true);
        pic.add_css_class("metis-window-thumb");
        return pic.upcast();
    }
    let img = gtk::Image::from_gicon(&crate::services::applications::resolve_icon_for_app_id(
        w.app_id.as_deref(),
    ));
    img.set_pixel_size(icon_px);
    img.upcast()
}
