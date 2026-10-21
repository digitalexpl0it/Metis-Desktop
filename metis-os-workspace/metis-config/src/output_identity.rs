//! Stable display identity for topology profiles (Phase 20 §C).
//!
//! Prefer EDID `make|model|serial`. When EDID is missing (winit, broken EDID),
//! fall back to the compositor connector name so profiles still round-trip.

/// Build a stable monitor identity string.
///
/// Empty / "Unknown" make/model and empty serial collapse to `connector_fallback`
/// so nested winit and incomplete EDID stay usable.
pub fn output_identity(make: &str, model: &str, serial: &str, connector_fallback: &str) -> String {
    let make = normalize_field(make);
    let model = normalize_field(model);
    let serial = normalize_field(serial);
    if make.is_empty() && model.is_empty() && serial.is_empty() {
        return connector_fallback.trim().to_string();
    }
    // If EDID make/model are Unknown placeholders with no serial, prefer connector.
    if serial.is_empty()
        && (make.is_empty() || make.eq_ignore_ascii_case("unknown"))
        && (model.is_empty() || model.eq_ignore_ascii_case("unknown"))
    {
        return connector_fallback.trim().to_string();
    }
    format!("{make}|{model}|{serial}")
}

fn normalize_field(raw: &str) -> String {
    raw.trim().replace('|', "/")
}

/// Sorted unique identity set joined for exact topology matching.
pub fn topology_fingerprint(identities: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut ids: Vec<String> = identities
        .into_iter()
        .map(|s| s.as_ref().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    ids.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_identity() {
        assert_eq!(
            output_identity("Dell", "U2720Q", "ABC123", "DP-1"),
            "Dell|U2720Q|ABC123"
        );
    }

    #[test]
    fn fallback_when_empty() {
        assert_eq!(output_identity("", "", "", "eDP-1"), "eDP-1");
        assert_eq!(
            output_identity("Unknown", "Unknown", "", "HDMI-A-1"),
            "HDMI-A-1"
        );
    }

    #[test]
    fn fingerprint_order_independent() {
        let a = topology_fingerprint(["b|m|1", "a|m|2"]);
        let b = topology_fingerprint(["a|m|2", "b|m|1", "a|m|2"]);
        assert_eq!(a, b);
        assert_eq!(a, "a|m|2\nb|m|1");
    }
}
