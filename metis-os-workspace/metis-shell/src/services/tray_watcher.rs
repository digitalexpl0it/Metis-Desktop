//! StatusNotifierWatcher + host client (zbus 5), replacing the stray crate.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use zbus::fdo::RequestNameFlags;
use zbus::interface;
use zbus::proxy;
use zbus::zvariant::{OwnedValue, Value};

use super::tray::{TrayCommand, TrayEvent, TrayItem};
use super::tray_dbus_types::{
    MenuLayout, ServiceParts, item_unique_name, parse_item_props, resolve_tray_display_title,
    service_parts,
};
use super::tray_menu::{MenuItem, TrayMenu, parse_menu_layout};

const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";

pub async fn run(
    event_tx: std::sync::mpsc::Sender<TrayEvent>,
    mut cmd_rx: tokio::sync::mpsc::Receiver<TrayCommand>,
) -> zbus::Result<()> {
    let (item_tx, mut item_rx) = mpsc::unbounded_channel();
    let state = Arc::new(Mutex::new(WatcherState::default()));
    let watcher = WatcherService {
        state: state.clone(),
        item_tx: item_tx.clone(),
    };

    let conn = zbus::connection::Builder::session()?
        .serve_at(WATCHER_PATH, watcher)?
        .build()
        .await?;

    let flags = RequestNameFlags::ReplaceExisting | RequestNameFlags::AllowReplacement;
    match conn.request_name_with_flags(WATCHER_NAME, flags).await {
        Ok(_) => tracing::info!("tray: acquired {WATCHER_NAME}"),
        Err(err) => {
            tracing::warn!(
                %err,
                "tray: could not own {WATCHER_NAME}; using existing watcher as host"
            );
        }
    }

    register_host(&conn).await?;

    let conn = Arc::new(conn);
    let event_tx_loop = event_tx.clone();
    let event_loop = {
        let conn = conn.clone();
        async move {
            while let Some(msg) = item_rx.recv().await {
                match msg {
                    ItemMsg::Update(parts) => match fetch_item(&conn, &parts).await {
                        Some(item) => {
                            tracing::info!(
                                id = %item.id,
                                title = %item.title,
                                "tray: item updated"
                            );
                            let _ = event_tx_loop.send(TrayEvent::Update(item));
                        }
                        None => {
                            tracing::warn!(
                                bus = %parts.bus_name,
                                path = %parts.object_path,
                                "tray: failed to fetch item properties"
                            );
                        }
                    },
                    ItemMsg::Remove { bus_name } => {
                        tracing::info!(%bus_name, "tray: item removed");
                        let _ = event_tx_loop.send(TrayEvent::Remove { bus_name });
                    }
                }
            }
        }
    };

    let item_tx_sync = item_tx.clone();
    let conn_sync = conn.clone();
    let event_tx_cmd = event_tx.clone();
    let cmd_loop = async move {
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TrayCommand::SyncRegistered => {
                    sync_registered_items(&conn_sync, &item_tx_sync).await;
                }
                TrayCommand::OpenContextMenu {
                    bus_name,
                    object_path,
                } => {
                    let conn = conn_sync.clone();
                    let event_tx = event_tx_cmd.clone();
                    let parts = ServiceParts {
                        bus_name,
                        object_path,
                    };
                    tokio::spawn(async move {
                        if let Err(err) = refresh_context_menu(&conn, &parts, &event_tx).await {
                            tracing::warn!(%err, "tray: context menu fetch failed");
                        }
                    });
                }
                other => {
                    let conn = conn_sync.clone();
                    tokio::spawn(async move {
                        if let Err(err) = dispatch_command(&conn, other).await {
                            tracing::warn!(%err, "tray: command failed");
                        }
                    });
                }
            }
        }
    };

    spawn_item_listener(conn.clone(), item_tx.clone());
    spawn_name_owner_listener(conn.clone(), state.clone(), item_tx.clone());
    sync_registered_items(&conn, &item_tx).await;

    tokio::select! {
        _ = event_loop => {}
        _ = cmd_loop => {}
    }

    Ok(())
}

#[derive(Default)]
struct WatcherState {
    hosts: HashSet<String>,
    items: HashSet<String>,
    host_registered: bool,
}

struct WatcherService {
    state: Arc<Mutex<WatcherState>>,
    item_tx: mpsc::UnboundedSender<ItemMsg>,
}

#[derive(Clone, Debug)]
enum ItemMsg {
    Update(ServiceParts),
    Remove { bus_name: String },
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherService {
    async fn register_status_notifier_host(&self, service: &str) {
        tracing::info!(host = service, "tray: host registered");
        let mut state = self.state.lock().expect("watcher state lock");
        state.hosts.insert(service.to_string());
        state.host_registered = true;
    }

    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) {
        let Some(sender) = header.sender() else {
            return;
        };
        let unique = sender.to_string();
        let item_id = format!("{unique}{service}");
        tracing::info!(item = %item_id, "tray: item registered");
        {
            let mut state = self.state.lock().expect("watcher state lock");
            state.items.insert(item_id.clone());
        }
        if let Some(parts) = service_parts(&item_id) {
            let _ = self.item_tx.send(ItemMsg::Update(parts));
        }
    }

    async fn unregister_status_notifier_item(&self, service: &str) {
        tracing::info!(service, "tray: item unregistered");
        let mut state = self.state.lock().expect("watcher state lock");
        state.items.retain(|item| !item.contains(service));
        let _ = self.item_tx.send(ItemMsg::Remove {
            bus_name: service.to_string(),
        });
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("watcher state lock")
            .items
            .iter()
            .cloned()
            .collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        self.state
            .lock()
            .expect("watcher state lock")
            .host_registered
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

async fn register_host(conn: &zbus::Connection) -> zbus::Result<()> {
    let pid = std::process::id();
    let host_name = format!("org.freedesktop.StatusNotifierHost-{pid}-metis-bar");
    conn.request_name_with_flags(
        host_name.as_str(),
        RequestNameFlags::AllowReplacement.into(),
    )
    .await?;
    let proxy = WatcherProxy::builder(conn)
        .destination(WATCHER_NAME)?
        .path(WATCHER_PATH)?
        .build()
        .await?;
    proxy
        .register_status_notifier_host(host_name.as_str())
        .await?;
    tracing::info!(host = host_name, "tray: registered StatusNotifierHost");
    Ok(())
}

async fn sync_registered_items(conn: &zbus::Connection, item_tx: &mpsc::UnboundedSender<ItemMsg>) {
    let proxy = match watcher_proxy(conn).await {
        Some(proxy) => proxy,
        None => return,
    };
    let Ok(items) = proxy.registered_status_notifier_items().await else {
        return;
    };
    tracing::info!(count = items.len(), "tray: syncing registered items");
    for item_id in items {
        if let Some(parts) = service_parts(&item_id) {
            let _ = item_tx.send(ItemMsg::Update(parts));
        } else {
            tracing::warn!(item_id, "tray: could not parse registered item id");
        }
    }
}

async fn watcher_proxy(conn: &zbus::Connection) -> Option<WatcherProxy<'_>> {
    WatcherProxy::builder(conn)
        .destination(WATCHER_NAME)
        .ok()?
        .path(WATCHER_PATH)
        .ok()?
        .build()
        .await
        .ok()
}

fn spawn_item_listener(conn: Arc<zbus::Connection>, item_tx: mpsc::UnboundedSender<ItemMsg>) {
    tokio::spawn(async move {
        let proxy = match watcher_proxy(&conn).await {
            Some(proxy) => proxy,
            None => return,
        };
        let Ok(mut stream) = proxy.receive_status_notifier_item_registered().await else {
            return;
        };
        while let Some(signal) = stream.next().await {
            let Ok(args) = signal.args() else {
                continue;
            };
            tracing::info!(
                service = args.service(),
                "tray: StatusNotifierItemRegistered"
            );
            sync_registered_items(&conn, &item_tx).await;
        }
    });
}

/// Most SNI apps (Electron/Chromium, Qt, …) never call
/// `UnregisterStatusNotifierItem` on exit — they just drop their D-Bus
/// connection. Without watching `NameOwnerChanged` the item lingers forever
/// (and every later property/menu fetch fails with `ServiceUnknown`). When a
/// unique connection name that backs a tray item loses its owner, drop the item.
fn spawn_name_owner_listener(
    conn: Arc<zbus::Connection>,
    state: Arc<Mutex<WatcherState>>,
    item_tx: mpsc::UnboundedSender<ItemMsg>,
) {
    tokio::spawn(async move {
        let dbus = match zbus::fdo::DBusProxy::new(&conn).await {
            Ok(proxy) => proxy,
            Err(err) => {
                tracing::warn!(%err, "tray: DBus proxy for NameOwnerChanged failed");
                return;
            }
        };
        let mut stream = match dbus.receive_name_owner_changed().await {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(%err, "tray: NameOwnerChanged subscribe failed");
                return;
            }
        };
        while let Some(signal) = stream.next().await {
            let Ok(args) = signal.args() else {
                continue;
            };
            // Only a *released* name (empty new owner) drops an item; acquisitions
            // and ownership transfers keep it.
            if args.new_owner().is_some() {
                continue;
            }
            let name = args.name().to_string();
            // An item is owned by `name` when it registered under that unique
            // connection (Claude: `:1.55:1.55`) or advertised it as its
            // destination well-known name (Flameshot: `:1.241org.kde...`). Match
            // either, and report the item's *destination* bus name so the UI —
            // which keys tray items by that — actually drops it.
            let removed: Vec<String> = {
                let mut state = state.lock().expect("watcher state lock");
                let mut removed = Vec::new();
                state.items.retain(|item| {
                    let dest = service_parts(item).map(|p| p.bus_name);
                    let owned = item_unique_name(item).as_deref() == Some(name.as_str())
                        || dest.as_deref() == Some(name.as_str());
                    if owned {
                        if let Some(dest) = dest {
                            removed.push(dest);
                        }
                        false
                    } else {
                        true
                    }
                });
                removed
            };
            for bus_name in removed {
                tracing::info!(%bus_name, vanished = %name, "tray: item owner vanished — removing");
                let _ = item_tx.send(ItemMsg::Remove { bus_name });
            }
        }
    });
}

async fn fetch_item(conn: &zbus::Connection, parts: &ServiceParts) -> Option<TrayItem> {
    let props = PropertiesProxy::builder(conn)
        .destination(parts.bus_name.as_str())
        .ok()?
        .path(parts.object_path.as_str())
        .ok()?
        .build()
        .await
        .ok()?
        .get_all("org.kde.StatusNotifierItem")
        .await
        .ok()?;

    let parsed = parse_item_props(&props);
    let menu_path = parsed.menu.clone();
    let menu = if let Some(path) = menu_path.clone() {
        fetch_menu(conn, &parts.bus_name, path.as_str()).await
    } else {
        None
    };

    let title = resolve_tray_display_title(
        &parsed.title,
        &parsed.id,
        &parsed.tooltip_title,
        &parsed.tooltip_subtitle,
        &parts.bus_name,
    );

    Some(TrayItem {
        bus_name: parts.bus_name.clone(),
        object_path: parts.object_path.clone(),
        id: parsed.id,
        title,
        icon_name: parsed.icon_name,
        icon_theme_path: parsed.icon_theme_path,
        icon_pixmap: parsed.icon_pixmap.map(|p| super::tray::IconPixmap {
            width: p.width,
            height: p.height,
            pixels: p.pixels,
        }),
        menu_path: menu_path.clone(),
        menu,
        item_is_menu: parsed.item_is_menu,
    })
}

const MENU_PROPS: &[&str] = &["label", "enabled", "visible", "type", "children-display"];

async fn fetch_menu(conn: &zbus::Connection, bus_name: &str, menu_path: &str) -> Option<TrayMenu> {
    let proxy = DBusMenuProxy::builder(conn)
        .destination(bus_name)
        .ok()?
        .path(menu_path)
        .ok()?
        .build()
        .await
        .ok()?;

    match proxy.about_to_show(0).await {
        Ok(true) => {
            // Steam/Qt rebuild their dbusmenu tree *asynchronously* after
            // AboutToShow. Fetching the layout immediately then returns a stale
            // (or empty) tree, so the parsed "Exit"/"Quit" item carries a dead id
            // — which is exactly why the first right-click's Quit does nothing and
            // only the second attempt (after the menu has settled) works. Give the
            // client a brief window to publish the updated layout before reading
            // it, so the very first open already has live ids.
            tracing::debug!(
                bus = bus_name,
                "tray: menu AboutToShow requested refresh — waiting for layout to settle"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(false) => {}
        Err(err) => {
            tracing::debug!(%err, bus = bus_name, "tray: AboutToShow failed (non-fatal)");
        }
    }

    let layout: MenuLayout = match proxy.get_layout(0, 10, MENU_PROPS).await {
        Ok(layout) => layout,
        Err(err) => {
            tracing::warn!(
                %err,
                bus = bus_name,
                path = menu_path,
                "tray: failed to fetch context menu layout"
            );
            return None;
        }
    };
    let menu = parse_menu_layout(layout);
    tracing::info!(
        bus = bus_name,
        entries = menu.submenus.len(),
        "tray: parsed context menu"
    );
    // Dump the item ids/labels so a failing "Exit"/"Quit" click can be traced to
    // the exact dbusmenu id the client exposes (some apps, notably Steam, number
    // items unexpectedly or gate them behind AboutToShow).
    if tracing::enabled!(tracing::Level::DEBUG) {
        log_menu_items(bus_name, &menu.submenus, 0);
    }
    Some(menu)
}

/// Re-fetch the live dbusmenu layout and resolve the current id of the item
/// whose label matches `label`. Steam renumbers ids on every rebuild, so the id
/// captured when the menu was displayed goes stale; matching on the (stable)
/// label recovers the current id. Returns `None` if the label isn't found (the
/// caller then falls back to the id the UI sent).
async fn resolve_menu_id_by_label(proxy: &DBusMenuProxy<'_>, label: &str) -> Option<i32> {
    if label.trim().is_empty() {
        return None;
    }
    // `AboutToShow` is what makes the client (re)publish its tree; let the async
    // rebuild settle before reading, so we parse the *current* layout.
    match proxy.about_to_show(0).await {
        Ok(true) => tokio::time::sleep(Duration::from_millis(150)).await,
        Ok(false) => {}
        Err(err) => tracing::debug!(%err, "tray: AboutToShow before click failed (non-fatal)"),
    }
    let layout: MenuLayout = proxy.get_layout(0, 10, MENU_PROPS).await.ok()?;
    let menu = parse_menu_layout(layout);
    find_menu_id_by_label(&menu.submenus, label)
}

/// Depth-first search for a menu item whose label matches `target`, comparing
/// case-insensitively and ignoring `_`/`&` mnemonic markers and surrounding
/// whitespace so "E_xit" matches "Exit".
fn find_menu_id_by_label(items: &[MenuItem], target: &str) -> Option<i32> {
    let normalize = |s: &str| -> String {
        s.chars()
            .filter(|c| *c != '_' && *c != '&')
            .collect::<String>()
            .trim()
            .to_ascii_lowercase()
    };
    let want = normalize(target);
    for item in items {
        if !item.label.is_empty() && normalize(&item.label) == want {
            return Some(item.id);
        }
        if let Some(found) = find_menu_id_by_label(&item.submenu, target) {
            return Some(found);
        }
    }
    None
}

fn log_menu_items(bus_name: &str, items: &[MenuItem], depth: usize) {
    for item in items {
        tracing::debug!(
            bus = bus_name,
            depth,
            id = item.id,
            label = %item.label,
            enabled = item.enabled,
            visible = item.visible,
            "tray: menu item"
        );
        if !item.submenu.is_empty() {
            log_menu_items(bus_name, &item.submenu, depth + 1);
        }
    }
}

async fn refresh_context_menu(
    conn: &zbus::Connection,
    parts: &ServiceParts,
    event_tx: &std::sync::mpsc::Sender<TrayEvent>,
) -> Result<(), String> {
    let mut item = fetch_item(conn, parts)
        .await
        .ok_or_else(|| "tray item not found".to_string())?;
    if item.menu_path.is_none() {
        return Err("tray item has no menu path".into());
    }
    let menu_path = item.menu_path.clone().unwrap();
    item.menu = fetch_menu(conn, &parts.bus_name, menu_path.as_str()).await;
    let _ = event_tx.send(TrayEvent::ContextMenuReady(item));
    Ok(())
}

async fn dispatch_command(conn: &zbus::Connection, cmd: TrayCommand) -> Result<(), String> {
    match cmd {
        TrayCommand::MenuClicked {
            bus_name,
            menu_path,
            submenu_id,
            label,
        } => {
            let proxy = DBusMenuProxy::builder(conn)
                .destination(bus_name.as_str())
                .map_err(|e| e.to_string())?
                .path(menu_path.as_str())
                .map_err(|e| e.to_string())?
                .build()
                .await
                .map_err(|e| e.to_string())?;

            // Steam (and other ayatana→dbusmenu bridges) *renumber* their menu
            // item ids every time the tree is rebuilt, so the id captured when the
            // menu was shown is routinely dead by the time of the click ("The ID
            // supplied N does not refer to a menu item we have"). Re-fetch the live
            // layout right now and re-resolve the id by its (stable) label; only if
            // that fails do we fall back to the id the UI sent. `AboutToShow` is
            // what triggers the rebuild, so we call it, let it settle, then read
            // the *post-rebuild* layout — the ids we resolve are therefore current.
            let deliver = |target: i32| {
                let proxy = proxy.clone();
                async move {
                    let ts = chrono::Utc::now().timestamp_millis();
                    proxy
                        .event(target, "clicked", &Value::from(0i32), ts as u32)
                        .await
                        .map_err(|e| e.to_string())
                }
            };

            let resolved = resolve_menu_id_by_label(&proxy, &label).await;
            let target = resolved.unwrap_or(submenu_id);
            tracing::debug!(
                bus = %bus_name,
                submenu_id,
                resolved = ?resolved,
                target,
                label = %label,
                "tray: dispatching menu click"
            );

            let mut result = deliver(target).await;
            // One retry: if the resolved id was *also* stale (the client renumbered
            // between our layout read and the click), re-resolve once and retry.
            if result.is_err()
                && let Some(fresh) = resolve_menu_id_by_label(&proxy, &label).await
                && fresh != target
            {
                tracing::debug!(
                    bus = %bus_name,
                    fresh,
                    "tray: first click failed — retrying with re-resolved id"
                );
                result = deliver(fresh).await;
            }

            match &result {
                Ok(()) => tracing::info!(
                    bus = %bus_name,
                    target,
                    label = %label,
                    "tray: menu click delivered"
                ),
                Err(err) => tracing::warn!(
                    bus = %bus_name,
                    target,
                    label = %label,
                    %err,
                    "tray: menu click failed"
                ),
            }
            result
        }
        TrayCommand::Activate {
            bus_name,
            object_path,
            x,
            y,
        } => {
            let proxy = StatusNotifierItemProxy::builder(conn)
                .destination(bus_name.as_str())
                .map_err(|e| e.to_string())?
                .path(object_path.as_str())
                .map_err(|e| e.to_string())?
                .build()
                .await
                .map_err(|e| e.to_string())?;
            if proxy.item_is_menu().await.unwrap_or(false) {
                sni_context_menu(&proxy, x, y).await
            } else {
                // Some items (notably Steam) advertise a tray icon but do not
                // implement `Activate`. Rather than erroring on every left-click,
                // fall back to opening the context menu so the icon stays useful
                // (Exit/Quit is reachable from there).
                match sni_activate(&proxy, x, y, true).await {
                    Ok(()) => Ok(()),
                    Err(err) => {
                        tracing::debug!(
                            bus = %bus_name,
                            %err,
                            "tray: Activate unsupported — falling back to context menu"
                        );
                        sni_context_menu(&proxy, x, y).await
                    }
                }
            }
        }
        TrayCommand::SecondaryActivate {
            bus_name,
            object_path,
            x,
            y,
        } => {
            let proxy = StatusNotifierItemProxy::builder(conn)
                .destination(bus_name.as_str())
                .map_err(|e| e.to_string())?
                .path(object_path.as_str())
                .map_err(|e| e.to_string())?
                .build()
                .await
                .map_err(|e| e.to_string())?;
            if sni_context_menu(&proxy, x, y).await.is_err() {
                sni_activate(&proxy, x, y, false).await?;
            }
            Ok(())
        }
        TrayCommand::SyncRegistered | TrayCommand::OpenContextMenu { .. } => Ok(()),
    }
}

async fn sni_context_menu(
    proxy: &StatusNotifierItemProxy<'_>,
    x: i32,
    y: i32,
) -> Result<(), String> {
    timeout(Duration::from_secs(3), proxy.context_menu(x, y))
        .await
        .map_err(|_| "tray context menu timed out".to_string())?
        .map_err(|e| e.to_string())
}

async fn sni_activate(
    proxy: &StatusNotifierItemProxy<'_>,
    x: i32,
    y: i32,
    activate: bool,
) -> Result<(), String> {
    if activate {
        timeout(Duration::from_secs(3), proxy.activate(x, y))
            .await
            .map_err(|_| "tray activate timed out".to_string())?
            .map_err(|e| e.to_string())
    } else {
        timeout(Duration::from_secs(3), proxy.secondary_activate(x, y))
            .await
            .map_err(|_| "tray secondary activate timed out".to_string())?
            .map_err(|e| e.to_string())
    }
}

#[proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
trait Watcher {
    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> zbus::Result<bool>;
    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: &str) -> zbus::Result<()>;
}

#[proxy(interface = "org.freedesktop.DBus.Properties", assume_defaults = true)]
trait Properties {
    #[zbus(name = "GetAll")]
    fn get_all(&self, interface_name: &str) -> zbus::Result<HashMap<String, OwnedValue>>;
}

#[proxy(interface = "com.canonical.dbusmenu", assume_defaults = true)]
trait DBusMenu {
    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<MenuLayout>;
    fn event(&self, id: i32, event_id: &str, data: &Value<'_>, timestamp: u32) -> zbus::Result<()>;
}

#[proxy(interface = "org.kde.StatusNotifierItem", assume_defaults = true)]
trait StatusNotifierItem {
    #[zbus(property)]
    fn item_is_menu(&self) -> zbus::Result<bool>;
    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn provide_xdg_activation_token(&self, token: &str) -> zbus::Result<()>;
}
