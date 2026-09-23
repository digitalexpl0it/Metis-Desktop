pub mod applications;
pub mod audio_viz;
pub mod calendar;
mod clipboard;
mod dashboard;
pub mod hardware;
pub mod launch_pending;
mod notifications;
mod notify_dbus;
mod poll;
pub mod secrets;
mod tray;
mod tray_dbus_types;
mod tray_menu;
mod tray_watcher;
mod updates;
mod volumes;
pub mod weather;
pub mod windows;
mod workspaces;

pub use applications::{AppEntry, watch_app_index};
pub use audio_viz::{audio_viz_frame, ensure_audio_viz, release_audio_viz};
pub use calendar::{
    CalCommand, Event as CalendarEvent, LocalEvent, reload_calendars, spawn_calendar_service,
};
pub use clipboard::{
    ClipboardEntry, ClipboardPage, active_entry_id, apply_clipboard_event, clear_history,
    delete_entry, filtered_entries, load_history, page_size, private_mode, recall_entry,
    register_refresh as register_clipboard_refresh, set_page_size, set_private_mode,
    toggle_favorite,
};
pub use dashboard::{
    DashboardSnapshot, DiskMount, GpuTempReading, ProcessClass, ProcessRow, format_bytes,
    format_rate, format_uptime, kill_process, kill_process_tree, set_polling_active,
    short_kernel_version, spawn_dashboard_pollers,
};
pub use notifications::{
    BarNotification, NotificationEntry, NotificationKind, clear_notifications, close_notification,
    dismiss_notification, dismiss_notification_by_dbus_id, do_not_disturb, invoke_action,
    notification_count, notify_store_changed, play_notification_sound, push_notification,
    register_refresh, runtime_notifications, set_action_sender, set_do_not_disturb,
};
pub use notify_dbus::{NotifyChannels, NotifyIncoming, spawn_notification_service};
pub use poll::{
    BarSnapshot, BluetoothDevice, BluetoothStatus, EthernetStatus, VpnFeedback, VpnStatus,
    WifiNetwork, arm_user_wifi_radio_toggle, bluetooth_set_powered, network_snapshot_for_ui,
    set_mic_mute, set_mic_volume_absolute, set_mute, set_volume_absolute, set_volume_relative,
    spawn_bar_pollers, suppress_wifi_radio_writes, vpn_clear_password_prompt, vpn_down, vpn_up,
    vpn_up_with_password, wifi_connect, wifi_ensure_radio_on, wifi_hotplug_suppressed, wifi_scan,
    wifi_set_radio,
};
pub use tray::{
    IconPixmap, TrayCommand, TrayEvent, TrayItem, TraySnapshot, apply_event,
    register_context_menu_ready, register_refresh as register_tray_refresh, send_command,
    set_command_sender, snapshot as tray_snapshot, spawn_tray_service, sync_tray,
};
pub use tray_menu::{MenuItem as TrayMenuItem, MenuType, TrayMenu};
pub use updates::{
    handle_notification_action as updates_handle_notification_action,
    is_visible as updates_is_visible, pending_count as updates_pending_count,
    register_refresh as register_updates_refresh,
    register_updater_refresh as register_updates_updater_refresh,
    request_check as updates_request_check, snapshot as updates_snapshot,
    snooze_hours as updates_snooze_hours, snooze_one_day as updates_snooze_one_day,
    snooze_tonight as updates_snooze_tonight, spawn_updates_service,
    start_apply as updates_start_apply,
};
pub use volumes::{
    VolumeEntry, VolumeKind, activate as volumes_activate, eject as volumes_eject,
    icon_name as volumes_icon_name, mount_volume as volumes_mount, open_in_file_manager,
    register_refresh as register_volumes_refresh, snapshot as volumes_snapshot,
    unmount as volumes_unmount,
};
pub use weather::{
    LocationWeather, WeatherSnapshot, last_weather_snapshot, spawn_weather_service, weather_refresh,
};
pub use windows::refresh_taskbars;
pub use workspaces::{
    WorkspaceSnapshot, active_workspace_for, dispatch_workspace, set_active_workspace,
    workspace_count, workspace_snapshot, workspace_snapshot_for,
};
