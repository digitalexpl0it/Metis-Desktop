//! BlueZ inventory / power via ObjectManager (no `bluetoothctl` forks).

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// Adapter + connected devices from `org.bluez` GetManagedObjects.
#[derive(Debug, Clone, Default)]
pub struct BluezSnapshot {
    pub adapter_present: bool,
    pub powered: bool,
    pub adapter_path: Option<String>,
    pub devices: Vec<BluezDevice>,
}

#[derive(Debug, Clone)]
pub struct BluezDevice {
    pub address: String,
    pub name: String,
    pub battery_percent: Option<u8>,
}

pub async fn read_snapshot() -> BluezSnapshot {
    let Ok(conn) = zbus::Connection::system().await else {
        return BluezSnapshot::default();
    };
    let Ok(proxy) = zbus::Proxy::new(
        &conn,
        "org.bluez",
        "/",
        "org.freedesktop.DBus.ObjectManager",
    )
    .await
    else {
        return BluezSnapshot::default();
    };
    let managed: HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>> =
        match proxy.call("GetManagedObjects", &()).await {
            Ok(m) => m,
            Err(err) => {
                tracing::debug!(%err, "bluez GetManagedObjects failed");
                return BluezSnapshot::default();
            }
        };

    let mut snap = BluezSnapshot::default();
    let mut battery_by_path: HashMap<String, u8> = HashMap::new();

    for (path, ifaces) in &managed {
        if let Some(batt) = ifaces.get("org.bluez.Battery1")
            && let Some(pct) = prop_u8(batt, "Percentage")
        {
            battery_by_path.insert(path.as_str().to_string(), pct);
        }
    }

    for (path, ifaces) in &managed {
        if let Some(adapter) = ifaces.get("org.bluez.Adapter1") {
            snap.adapter_present = true;
            snap.powered = prop_bool(adapter, "Powered").unwrap_or(false);
            snap.adapter_path = Some(path.as_str().to_string());
        }
    }

    for (path, ifaces) in &managed {
        let Some(dev) = ifaces.get("org.bluez.Device1") else {
            continue;
        };
        if !prop_bool(dev, "Connected").unwrap_or(false) {
            continue;
        }
        let address = prop_string(dev, "Address").unwrap_or_default();
        if address.is_empty() {
            continue;
        }
        let name = prop_string(dev, "Alias")
            .or_else(|| prop_string(dev, "Name"))
            .unwrap_or_else(|| address.clone());
        let battery_percent = battery_by_path.get(path.as_str()).copied().or_else(|| {
            // Battery1 sometimes lives on the same object.
            ifaces
                .get("org.bluez.Battery1")
                .and_then(|b| prop_u8(b, "Percentage"))
        });
        snap.devices.push(BluezDevice {
            address,
            name,
            battery_percent,
        });
    }

    snap
}

pub async fn set_powered(on: bool) -> Result<(), String> {
    let snap = read_snapshot().await;
    let path = snap
        .adapter_path
        .ok_or_else(|| "no BlueZ adapter".to_string())?;
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| e.to_string())?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.bluez",
        ObjectPath::try_from(path.as_str()).map_err(|e| e.to_string())?,
        "org.freedesktop.DBus.Properties",
    )
    .await
    .map_err(|e| e.to_string())?;
    let value = Value::from(on);
    proxy
        .call_method(
            "Set",
            &(
                "org.bluez.Adapter1",
                "Powered",
                OwnedValue::try_from(&value).map_err(|e| e.to_string())?,
            ),
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn prop_bool(map: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    map.get(key)
        .and_then(|v| bool::try_from(v.try_clone().ok()?).ok())
}

fn prop_string(map: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

fn prop_u8(map: &HashMap<String, OwnedValue>, key: &str) -> Option<u8> {
    map.get(key).and_then(|v| {
        let owned = v.try_clone().ok()?;
        if let Ok(b) = u8::try_from(owned.clone()) {
            return Some(b);
        }
        if let Ok(n) = u32::try_from(owned.clone()) {
            return Some(n.min(100) as u8);
        }
        if let Ok(n) = i64::try_from(owned) {
            return Some(n.clamp(0, 100) as u8);
        }
        None
    })
}
