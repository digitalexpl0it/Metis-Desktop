//! Named display topology profiles (`~/.config/metis/output-profiles.json`).
//!
//! Sibling to live [`outputs.json`](super::outputs) — profiles are keyed by EDID
//! identity sets and materialize into connector-keyed prefs on match / restore.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use super::output_identity::topology_fingerprint;
use super::outputs::{DisplayLayoutMode, OutputPrefs, OutputsConfig};
use super::session::unix_now_secs;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct OutputProfilesConfig {
    #[serde(default)]
    pub profiles: Vec<OutputTopologyProfile>,
    /// Last profile applied by hotplug or Settings Restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputTopologyProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub updated_at: u64,
    #[serde(default)]
    pub monitors: Vec<ProfileMonitor>,
    #[serde(default)]
    pub display_mode: DisplayLayoutMode,
    /// Identity of the mirror source when `display_mode` is Mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_source_identity: Option<String>,
    /// Identity of the primary output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileMonitor {
    pub identity: String,
    /// Last known connector name (hint only; remapped on apply).
    #[serde(default)]
    pub connector_hint: String,
    #[serde(default)]
    pub prefs: OutputPrefs,
}

pub fn output_profiles_config_path() -> std::path::PathBuf {
    super::config_dir().join("output-profiles.json")
}

pub fn load_output_profiles_config() -> OutputProfilesConfig {
    let path = output_profiles_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match serde_json::from_str::<OutputProfilesConfig>(&text) {
            Ok(cfg) => return cfg.sanitize(),
            Err(err) => tracing::warn!(%err, "output-profiles.json parse failed — using defaults"),
        }
    }
    OutputProfilesConfig::default()
}

pub fn save_output_profiles_config(cfg: &OutputProfilesConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    crate::persist::write_json_atomic(&output_profiles_config_path(), &cfg.clone().sanitize())
}

impl OutputProfilesConfig {
    pub fn sanitize(mut self) -> Self {
        self.profiles.retain(|p| {
            !p.id.trim().is_empty() && !p.name.trim().is_empty() && !p.monitors.is_empty()
        });
        let mut seen = HashSet::new();
        self.profiles.retain(|p| seen.insert(p.id.clone()));
        for p in &mut self.profiles {
            p.monitors.retain(|m| !m.identity.trim().is_empty());
            let mut mseen = HashSet::new();
            p.monitors.retain(|m| mseen.insert(m.identity.clone()));
        }
        self.profiles.retain(|p| !p.monitors.is_empty());
        if let Some(ref id) = self.last_applied_id
            && !self.profiles.iter().any(|p| &p.id == id)
        {
            self.last_applied_id = None;
        }
        self
    }

    pub fn fingerprint_for(profile: &OutputTopologyProfile) -> String {
        topology_fingerprint(profile.monitors.iter().map(|m| m.identity.as_str()))
    }

    /// Exact set match; on ties pick newest `updated_at`.
    pub fn find_matching_profile(
        &self,
        connected_identities: &[String],
    ) -> Option<&OutputTopologyProfile> {
        let want = topology_fingerprint(connected_identities.iter().map(|s| s.as_str()));
        if want.is_empty() {
            return None;
        }
        self.profiles
            .iter()
            .filter(|p| Self::fingerprint_for(p) == want)
            .max_by_key(|p| p.updated_at)
    }

    pub fn find_by_id(&self, id: &str) -> Option<&OutputTopologyProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn remove_by_id(&mut self, id: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != id);
        if self.last_applied_id.as_deref() == Some(id) {
            self.last_applied_id = None;
        }
        self.profiles.len() != before
    }
}

/// Allocate a new profile id (hex timestamp nanos — no uuid dependency).
pub fn new_profile_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// Build a profile snapshot from live outputs + connector→identity map.
///
/// `connected` is `(connector_name, identity)` for each currently connected output.
pub fn profile_from_live(
    name: String,
    outputs: &OutputsConfig,
    connected: &[(String, String)],
) -> OutputTopologyProfile {
    let mut monitors = Vec::new();
    for (connector, identity) in connected {
        let prefs = outputs.outputs.get(connector).cloned().unwrap_or_default();
        monitors.push(ProfileMonitor {
            identity: identity.clone(),
            connector_hint: connector.clone(),
            prefs,
        });
    }
    let identity_of = |connector: &str| -> Option<String> {
        connected
            .iter()
            .find(|(c, _)| c == connector)
            .map(|(_, id)| id.clone())
    };
    let primary_identity = outputs.primary_output.as_deref().and_then(identity_of);
    let mirror_source_identity = outputs.mirror_source.as_deref().and_then(identity_of);
    OutputTopologyProfile {
        id: new_profile_id(),
        name,
        updated_at: unix_now_secs(),
        monitors,
        display_mode: outputs.display_mode,
        mirror_source_identity,
        primary_identity,
    }
}

/// Materialize a profile into live `OutputsConfig`, remapping identities to the
/// current connector names. Preserves night-light globals from `base`.
///
/// `connector_by_identity` maps identity → current connector name.
pub fn materialize_to_outputs(
    profile: &OutputTopologyProfile,
    connector_by_identity: &HashMap<String, String>,
    base: &OutputsConfig,
) -> OutputsConfig {
    let mut cfg = base.clone();
    cfg.display_mode = profile.display_mode;

    cfg.primary_output = profile
        .primary_identity
        .as_ref()
        .and_then(|id| connector_by_identity.get(id).cloned());
    cfg.mirror_source = profile
        .mirror_source_identity
        .as_ref()
        .and_then(|id| connector_by_identity.get(id).cloned());

    // Drop prefs for connectors not in this topology so stale layout keys don't
    // fight auto-pack — keep other disconnected connectors' saved prefs.
    let active_connectors: HashSet<String> = connector_by_identity.values().cloned().collect();
    for mon in &profile.monitors {
        let Some(connector) = connector_by_identity.get(&mon.identity) else {
            continue;
        };
        cfg.outputs.insert(connector.clone(), mon.prefs.clone());
    }
    // Ensure every active connector has an entry (identity miss → defaults).
    for connector in &active_connectors {
        cfg.outputs.entry(connector.clone()).or_default();
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::OutputPrefs;

    fn mon(id: &str, x: i32) -> ProfileMonitor {
        ProfileMonitor {
            identity: id.into(),
            connector_hint: "hint".into(),
            prefs: OutputPrefs {
                layout_x: Some(x),
                layout_y: Some(0),
                ..OutputPrefs::default()
            },
        }
    }

    #[test]
    fn sanitize_is_idempotent() {
        let once = OutputProfilesConfig {
            profiles: vec![
                OutputTopologyProfile {
                    id: "a".into(),
                    name: "Home".into(),
                    updated_at: 1,
                    monitors: vec![mon("Dell|U|1", 0)],
                    display_mode: DisplayLayoutMode::Extend,
                    mirror_source_identity: None,
                    primary_identity: None,
                },
                OutputTopologyProfile {
                    id: "a".into(),
                    name: "dup".into(),
                    updated_at: 2,
                    monitors: vec![mon("Dell|U|1", 0)],
                    display_mode: DisplayLayoutMode::Extend,
                    mirror_source_identity: None,
                    primary_identity: None,
                },
                OutputTopologyProfile {
                    id: "".into(),
                    name: "bad".into(),
                    updated_at: 0,
                    monitors: vec![mon("x", 0)],
                    display_mode: DisplayLayoutMode::Extend,
                    mirror_source_identity: None,
                    primary_identity: None,
                },
            ],
            last_applied_id: Some("missing".into()),
        }
        .sanitize();
        let twice = once.clone().sanitize();
        assert_eq!(once, twice);
        assert_eq!(once.profiles.len(), 1);
        assert!(once.last_applied_id.is_none());
    }

    #[test]
    fn match_newest_on_tie() {
        let cfg = OutputProfilesConfig {
            profiles: vec![
                OutputTopologyProfile {
                    id: "old".into(),
                    name: "Old".into(),
                    updated_at: 10,
                    monitors: vec![mon("a|b|1", 0), mon("c|d|2", 1920)],
                    display_mode: DisplayLayoutMode::Extend,
                    mirror_source_identity: None,
                    primary_identity: None,
                },
                OutputTopologyProfile {
                    id: "new".into(),
                    name: "New".into(),
                    updated_at: 99,
                    monitors: vec![mon("c|d|2", 100), mon("a|b|1", 0)],
                    display_mode: DisplayLayoutMode::Extend,
                    mirror_source_identity: None,
                    primary_identity: None,
                },
            ],
            last_applied_id: None,
        };
        let hit = cfg
            .find_matching_profile(&["a|b|1".into(), "c|d|2".into()])
            .expect("match");
        assert_eq!(hit.id, "new");
    }

    #[test]
    fn materialize_remaps_connectors() {
        let profile = OutputTopologyProfile {
            id: "p".into(),
            name: "Dock".into(),
            updated_at: 1,
            monitors: vec![mon("id-laptop", 0), mon("id-ext", 1920)],
            display_mode: DisplayLayoutMode::Extend,
            mirror_source_identity: None,
            primary_identity: Some("id-laptop".into()),
        };
        let map: HashMap<String, String> = [
            ("id-laptop".into(), "eDP-1".into()),
            ("id-ext".into(), "DP-3".into()),
        ]
        .into_iter()
        .collect();
        let base = OutputsConfig {
            night_light_enabled: true,
            night_light_temperature: 3500,
            ..OutputsConfig::default()
        };
        let out = materialize_to_outputs(&profile, &map, &base);
        assert!(out.night_light_enabled);
        assert_eq!(out.night_light_temperature, 3500);
        assert_eq!(out.primary_output.as_deref(), Some("eDP-1"));
        assert_eq!(out.outputs["eDP-1"].layout_x, Some(0));
        assert_eq!(out.outputs["DP-3"].layout_x, Some(1920));
    }
}
