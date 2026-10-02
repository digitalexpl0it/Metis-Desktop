//! Settings → System → Metis Remote (RUDP streaming host).
//!
//! Classic RDP / GRD stays on the Remote access page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{RudpConfig, RudpEncoderBackend, RudpVideoCodec};
use metis_i18n::tr;
use metis_remote::AccountInfo;

use crate::{runtime, ui};

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("rudp");
    let cfg = Rc::new(RefCell::new(metis_config::load_rudp_config()));
    let toggling = Rc::new(Cell::new(false));

    let hint = gtk::Label::new(Some(&tr(
        "Metis Remote is the primary low-latency desktop stream (RUDP). It uses \
         local system accounts (PAM). Classic RDP and third-party tools stay under \
         Remote access.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    let (host_card, host_body) =
        ui::section_with_icon(&tr("Metis Remote"), "network-workgroup-symbolic");

    let enable = gtk::Switch::new();
    enable.set_active(cfg.borrow().enabled);
    enable.set_halign(gtk::Align::End);
    host_body.append(&ui::row(&tr("Allow Metis Remote connections"), &enable));

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("metis-settings-value");
    host_body.append(&readout_row(&tr("Status"), &status));

    let port = gtk::SpinButton::with_range(1024.0, 65535.0, 1.0);
    port.set_digits(0);
    port.set_value(f64::from(cfg.borrow().port));
    port.set_halign(gtk::Align::End);
    port.set_width_chars(6);
    host_body.append(&ui::row(&tr("UDP port"), &port));

    let lan = gtk::Switch::new();
    lan.set_active(cfg.borrow().lan_only);
    lan.set_halign(gtk::Align::End);
    lan.set_tooltip_text(Some(&tr(
        "When on, Metis applies nftables/ufw rules so only LAN/link-local can reach the UDP port.",
    )));
    host_body.append(&ui::row(&tr("LAN only (recommended)"), &lan));

    let conn_addr = gtk::Label::new(None);
    conn_addr.set_xalign(0.0);
    conn_addr.set_selectable(true);
    conn_addr.add_css_class("metis-settings-value");
    host_body.append(&readout_row(&tr("Connection address"), &conn_addr));

    let fingerprint = gtk::Label::new(None);
    fingerprint.set_xalign(0.0);
    fingerprint.set_selectable(true);
    fingerprint.set_wrap(true);
    fingerprint.add_css_class("metis-settings-value");
    host_body.append(&readout_row(&tr("Host fingerprint"), &fingerprint));

    let copy_fp = gtk::Button::with_label(&tr("Copy fingerprint"));
    copy_fp.set_halign(gtk::Align::End);
    host_body.append(&copy_fp);

    let connect_viewer = gtk::Button::with_label(&tr("Connect with Metis Viewer…"));
    connect_viewer.set_halign(gtk::Align::End);
    host_body.append(&connect_viewer);

    let fw_status = gtk::Label::new(None);
    fw_status.set_xalign(0.0);
    fw_status.set_wrap(true);
    fw_status.add_css_class("metis-settings-hint");
    host_body.append(&fw_status);

    let retry_fw = gtk::Button::with_label(&tr("Retry firewall apply"));
    retry_fw.set_halign(gtk::Align::End);
    host_body.append(&retry_fw);
    content.append(&host_card);

    let tofu_hint = gtk::Label::new(Some(&tr(
        "Clients pin this fingerprint on first connect (TOFU). Authentication uses \
         local PAM passwords for allowed accounts. Use Metis Viewer (Metis Remote \
         protocol) or: metis-rudp-smoke HOST:PORT --user NAME",
    )));
    tofu_hint.set_xalign(0.0);
    tofu_hint.set_wrap(true);
    tofu_hint.add_css_class("metis-settings-hint");
    content.append(&tofu_hint);

    let (enc_card, enc_body) =
        ui::section_with_icon(&tr("Hardware encode"), "video-display-symbolic");
    let enc_hint = gtk::Label::new(Some(&tr(
        "Auto picks NVENC on NVIDIA render nodes and VAAPI elsewhere. Prefer HEVC; \
         the host falls back to H.264 if the preferred codec cannot open. Requires \
         system FFmpeg with VAAPI and/or NVENC.",
    )));
    enc_hint.set_xalign(0.0);
    enc_hint.set_wrap(true);
    enc_hint.add_css_class("metis-settings-hint");
    enc_body.append(&enc_hint);

    let encoder_labels = [tr("Auto"), tr("VAAPI (Intel/AMD)"), tr("NVENC (NVIDIA)")];
    let encoder_refs: Vec<&str> = encoder_labels.iter().map(|s| s.as_str()).collect();
    let encoder_dd = gtk::DropDown::from_strings(&encoder_refs);
    encoder_dd.set_selected(encoder_backend_index(cfg.borrow().encoder));
    encoder_dd.set_halign(gtk::Align::End);
    enc_body.append(&ui::row(&tr("Encoder"), &encoder_dd));

    let codec_labels = [tr("HEVC (H.265)"), tr("H.264")];
    let codec_refs: Vec<&str> = codec_labels.iter().map(|s| s.as_str()).collect();
    let codec_dd = gtk::DropDown::from_strings(&codec_refs);
    codec_dd.set_selected(codec_index(cfg.borrow().codec));
    codec_dd.set_halign(gtk::Align::End);
    enc_body.append(&ui::row(&tr("Preferred codec"), &codec_dd));

    let bitrate = gtk::SpinButton::with_range(500.0, 200_000.0, 500.0);
    bitrate.set_digits(0);
    bitrate.set_value(f64::from(cfg.borrow().bitrate_kbps));
    bitrate.set_halign(gtk::Align::End);
    bitrate.set_width_chars(8);
    bitrate.set_tooltip_text(Some(&tr("Target bitrate in kilobits per second.")));
    enc_body.append(&ui::row(&tr("Bitrate (kbps)"), &bitrate));

    let encode_hint = gtk::Label::new(None);
    encode_hint.set_xalign(0.0);
    encode_hint.add_css_class("metis-settings-value");
    enc_body.append(&readout_row(&tr("Encode plan"), &encode_hint));
    content.append(&enc_card);

    let (acct_card, acct_body) =
        ui::section_with_icon(&tr("Allowed accounts"), "system-users-symbolic");
    let acct_hint = gtk::Label::new(Some(&tr(
        "Only listed local users may authenticate once PAM sign-in ships. \
         Turning Metis Remote on seeds your current account when the list is empty.",
    )));
    acct_hint.set_xalign(0.0);
    acct_hint.set_wrap(true);
    acct_hint.add_css_class("metis-settings-hint");
    acct_body.append(&acct_hint);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("metis-settings-list");
    acct_body.append(&list);
    content.append(&acct_card);

    let refresh_status = {
        let status = status.clone();
        let encode_hint = encode_hint.clone();
        let fingerprint = fingerprint.clone();
        let conn_addr = conn_addr.clone();
        let fw_status = fw_status.clone();
        let retry_fw = retry_fw.clone();
        let cfg = cfg.clone();
        Rc::new(move || {
            let c = cfg.borrow();
            if c.enabled {
                status.set_text(&format!(
                    "{} (UDP {})",
                    tr("Enabled — compositor listens when Metis is on DRM"),
                    c.port
                ));
            } else {
                status.set_text(&tr("Stopped"));
            }
            encode_hint.set_text(&encode_plan_text(&c));
            conn_addr.set_text(&rudp_connection_address(c.port));
            match metis_config::read_rudp_host_fingerprint() {
                Some(fp) => fingerprint.set_text(&fp),
                None => fingerprint.set_text(&tr(
                    "Not created yet — appears when the Metis Remote host starts on DRM",
                )),
            }
            let (fw_text, show_retry) = rudp_firewall_status_text(&c);
            fw_status.set_text(&fw_text);
            retry_fw.set_visible(show_retry);
        })
    };
    refresh_status();

    let persist: Rc<dyn Fn()> = {
        let cfg = cfg.clone();
        let refresh_status = refresh_status.clone();
        Rc::new(move || {
            let snapshot = cfg.borrow().clone();
            if let Err(err) = metis_config::save_rudp_config(&snapshot) {
                tracing::warn!(%err, "failed to save rudp.json");
                return;
            }
            sync_rudp_firewall(&snapshot);
            // Reload cfg firewall fields after sync.
            *cfg.borrow_mut() = metis_config::load_rudp_config();
            refresh_status();
            runtime::reload_rudp_async();
        })
    };

    {
        let fingerprint = fingerprint.clone();
        copy_fp.connect_clicked(move |_| {
            let text = fingerprint.text();
            if text.is_empty() || text.contains("Not created") {
                return;
            }
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(&text);
            }
        });
    }
    {
        let cfg = cfg.clone();
        let status = status.clone();
        connect_viewer.connect_clicked(move |_| {
            let port = cfg.borrow().port;
            let hint = rudp_connection_address(port);
            let (host, port) = crate::remote::parse_connection_hint(&hint);
            let user = std::env::var("USER").ok();
            match crate::remote::open_viewer_rudp(Some(&host), Some(port), user.as_deref()) {
                Ok(()) => status.set_text(&tr("Opening Metis Viewer (Metis Remote)…")),
                Err(err) => {
                    tracing::warn!(%err, "failed to open Metis Viewer for RUDP");
                    status.set_text(&err);
                }
            }
        });
    }
    {
        let cfg = cfg.clone();
        let refresh_status = refresh_status.clone();
        retry_fw.connect_clicked(move |_| {
            let c = cfg.borrow().clone();
            if c.enabled && c.lan_only {
                match metis_remote::firewall_rudp_apply() {
                    Ok(_) => {}
                    Err(err) => tracing::warn!(%err, "rudp firewall apply failed"),
                }
                *cfg.borrow_mut() = metis_config::load_rudp_config();
                refresh_status();
            }
        });
    }

    {
        let cfg = cfg.clone();
        let persist = persist.clone();
        let toggling = toggling.clone();
        let list = list.clone();
        enable.connect_active_notify(move |sw| {
            if toggling.get() {
                return;
            }
            let on = sw.is_active();
            {
                let mut c = cfg.borrow_mut();
                c.enabled = on;
                if on {
                    let user = std::env::var("USER").unwrap_or_default();
                    c.seed_current_user_if_needed(&user);
                }
            }
            rebuild_accounts(&list, &cfg, &persist, &toggling);
            persist();
        });
    }
    {
        let cfg = cfg.clone();
        let persist = persist.clone();
        port.connect_value_changed(move |sp| {
            let v = sp.value().round() as u16;
            cfg.borrow_mut().port = v;
            persist();
        });
    }
    {
        let cfg = cfg.clone();
        let persist = persist.clone();
        let toggling = toggling.clone();
        lan.connect_active_notify(move |sw| {
            if toggling.get() {
                return;
            }
            cfg.borrow_mut().lan_only = sw.is_active();
            persist();
        });
    }
    {
        let cfg = cfg.clone();
        let persist = persist.clone();
        encoder_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().encoder = encoder_backend_from_index(dd.selected());
            persist();
        });
    }
    {
        let cfg = cfg.clone();
        let persist = persist.clone();
        codec_dd.connect_selected_notify(move |dd| {
            cfg.borrow_mut().codec = codec_from_index(dd.selected());
            persist();
        });
    }
    {
        let cfg = cfg.clone();
        let persist = persist.clone();
        bitrate.connect_value_changed(move |sp| {
            let v = sp.value().round() as u32;
            cfg.borrow_mut().bitrate_kbps = v;
            persist();
        });
    }

    rebuild_accounts(&list, &cfg, &persist, &toggling);

    scroller.upcast()
}

fn encoder_backend_index(backend: RudpEncoderBackend) -> u32 {
    match backend {
        RudpEncoderBackend::Auto => 0,
        RudpEncoderBackend::Vaapi => 1,
        RudpEncoderBackend::Nvenc => 2,
    }
}

fn encoder_backend_from_index(idx: u32) -> RudpEncoderBackend {
    match idx {
        1 => RudpEncoderBackend::Vaapi,
        2 => RudpEncoderBackend::Nvenc,
        _ => RudpEncoderBackend::Auto,
    }
}

fn codec_index(codec: RudpVideoCodec) -> u32 {
    match codec {
        RudpVideoCodec::Hevc => 0,
        RudpVideoCodec::H264 => 1,
    }
}

fn codec_from_index(idx: u32) -> RudpVideoCodec {
    match idx {
        1 => RudpVideoCodec::H264,
        _ => RudpVideoCodec::Hevc,
    }
}

fn encode_plan_text(cfg: &RudpConfig) -> String {
    let backend = match cfg.encoder {
        RudpEncoderBackend::Auto => tr("Auto → NVENC on NVIDIA, else VAAPI"),
        RudpEncoderBackend::Vaapi => tr("VAAPI"),
        RudpEncoderBackend::Nvenc => tr("NVENC"),
    };
    let codec = match cfg.codec {
        RudpVideoCodec::Hevc => tr("prefer HEVC, fall back to H.264"),
        RudpVideoCodec::H264 => tr("prefer H.264, fall back to HEVC"),
    };
    format!("{backend}; {codec}; {} kbps", cfg.bitrate_kbps)
}

fn rudp_firewall_status_text(cfg: &RudpConfig) -> (String, bool) {
    if !cfg.lan_only {
        return (tr("LAN only is off — no Metis UDP firewall rules."), false);
    }
    if !cfg.enabled {
        return (
            tr("Enable Metis Remote to apply LAN-only UDP firewall rules."),
            false,
        );
    }
    if cfg.firewall_applied {
        let backend = if cfg.firewall_backend.is_empty() {
            "firewall".to_string()
        } else {
            cfg.firewall_backend.clone()
        };
        return (
            format!("{} ({backend})", tr("LAN-only UDP firewall applied")),
            false,
        );
    }
    let detail = cfg
        .firewall_last_error
        .clone()
        .unwrap_or_else(|| tr("LAN-only is on, but firewall rules are not applied yet."));
    (detail, true)
}

fn sync_rudp_firewall(cfg: &RudpConfig) {
    if cfg.enabled && cfg.lan_only {
        if let Err(err) = metis_remote::firewall_rudp_apply() {
            tracing::warn!(%err, "rudp firewall apply failed");
        }
    } else if let Err(err) = metis_remote::firewall_rudp_clear() {
        tracing::warn!(%err, "rudp firewall clear failed");
    }
}

fn readout_row(label: &str, value: &gtk::Label) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    name.add_css_class("metis-settings-label");
    row.append(&name);
    row.append(value);
    row
}

fn rebuild_accounts(
    list: &gtk::Box,
    cfg: &Rc<RefCell<RudpConfig>>,
    persist: &Rc<dyn Fn()>,
    toggling: &Rc<Cell<bool>>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let accounts = match metis_remote::list_accounts() {
        Ok(a) => a,
        Err(err) => {
            let err_lbl = gtk::Label::new(Some(&format!(
                "{} ({err})",
                tr("Could not list local users.")
            )));
            err_lbl.set_xalign(0.0);
            err_lbl.add_css_class("metis-settings-hint");
            list.append(&err_lbl);
            return;
        }
    };

    if accounts.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No local users found.")));
        empty.set_xalign(0.0);
        empty.add_css_class("metis-settings-hint");
        list.append(&empty);
        return;
    }

    for acct in accounts {
        list.append(&account_row(acct, cfg, persist, toggling));
    }
}

fn account_row(
    acct: AccountInfo,
    cfg: &Rc<RefCell<RudpConfig>>,
    persist: &Rc<dyn Fn()>,
    toggling: &Rc<Cell<bool>>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");

    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.set_hexpand(true);
    let title = if acct.display_name.trim().is_empty() || acct.display_name == acct.username {
        acct.username.clone()
    } else {
        format!("{} ({})", acct.display_name, acct.username)
    };
    let name = gtk::Label::new(Some(&title));
    name.set_xalign(0.0);
    name.add_css_class("metis-settings-label");
    col.append(&name);
    if acct.is_current {
        let tag = gtk::Label::new(Some(&tr("Current session")));
        tag.set_xalign(0.0);
        tag.add_css_class("metis-settings-hint");
        col.append(&tag);
    }
    row.append(&col);

    let sw = gtk::Switch::new();
    sw.set_halign(gtk::Align::End);
    sw.set_valign(gtk::Align::Center);
    {
        let allowed = cfg
            .borrow()
            .allowed_users
            .iter()
            .any(|u| u == &acct.username);
        toggling.set(true);
        sw.set_active(allowed);
        toggling.set(false);
    }

    let username = acct.username.clone();
    let cfg = cfg.clone();
    let persist = persist.clone();
    let toggling = toggling.clone();
    sw.connect_active_notify(move |sw| {
        if toggling.get() {
            return;
        }
        let on = sw.is_active();
        {
            let mut c = cfg.borrow_mut();
            if on {
                if !c.allowed_users.iter().any(|u| u == &username) {
                    c.allowed_users.push(username.clone());
                }
            } else {
                // Keep at least one allowed user while the host is enabled.
                if c.enabled
                    && c.allowed_users.len() <= 1
                    && c.allowed_users.iter().any(|u| u == &username)
                {
                    toggling.set(true);
                    sw.set_active(true);
                    toggling.set(false);
                    return;
                }
                c.allowed_users.retain(|u| u != &username);
            }
        }
        persist();
    });
    row.append(&sw);
    row
}

/// Prefer a LAN IPv4 from the RDP remote snapshot helpers, else hostname.
fn rudp_connection_address(port: u16) -> String {
    let snap = crate::remote::load_snapshot();
    let host = snap
        .addresses
        .iter()
        .find(|a| {
            let a = a.as_str();
            !a.starts_with("127.") && !a.contains(':') && !a.is_empty()
        })
        .cloned()
        .unwrap_or_else(|| snap.hostname.clone());
    format!("{host}:{port}")
}
