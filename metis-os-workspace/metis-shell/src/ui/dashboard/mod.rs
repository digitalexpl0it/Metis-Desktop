//! Pull-down / pull-aside system dashboard (Phase 10).
//!
//! The control center is a separate layer-shell surface attached just inside the
//! edge-bar pill. The bar strip itself never resizes when the panel opens.

mod charts;
mod lifecycle;
mod panel;
mod processes;
mod views;
mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use crate::services::DashboardSnapshot;
use crate::ui::bar::BarShell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ProcessClassFilter {
    #[default]
    All,
    UserApps,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ProcessSortColumn {
    #[default]
    Cpu,
    Name,
    Pid,
    User,
    Kind,
    Memory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SortDirection {
    #[default]
    Desc,
    Asc,
}

pub(crate) const SNAP_MS: u32 = 220;
pub(crate) const OPEN_THRESHOLD: f64 = 72.0;
pub(crate) const PULL_START_SLOP: f64 = 6.0;

thread_local! {
    static DASHBOARD: RefCell<Option<Rc<Dashboard>>> = const { RefCell::new(None) };
    static POLL_ATTACHED: Cell<bool> = const { Cell::new(false) };
    /// Open process-row context menu; list rebuild is paused while set.
    static PROCESS_CONTEXT_MENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
}

pub(crate) struct Dashboard {
    pub(crate) shell: BarShell,
    /// Dedicated layer-shell window — never shares geometry with the edge bar.
    pub(crate) window: gtk::Window,
    pub(crate) root: gtk::Box,
    pub(crate) header: gtk::Box,
    pub(crate) tab_switcher: gtk::StackSwitcher,
    pub(crate) stack: gtk::Stack,
    pub(crate) overview: views::OverviewPage,
    pub(crate) processes: views::ProcessPage,
    pub(crate) cpu_hist: Rc<RefCell<Vec<f32>>>,
    pub(crate) cpu_core_hist: Rc<RefCell<Vec<Vec<f32>>>>,
    pub(crate) mem_hist: Rc<RefCell<Vec<f32>>>,
    pub(crate) swap_hist: Rc<RefCell<Vec<f32>>>,
    pub(crate) has_swap: Rc<Cell<bool>>,
    pub(crate) rx_hist: Rc<RefCell<Vec<f64>>>,
    pub(crate) tx_hist: Rc<RefCell<Vec<f64>>>,
    pub(crate) disk_read_hist: Rc<RefCell<Vec<f64>>>,
    pub(crate) disk_write_hist: Rc<RefCell<Vec<f64>>>,
    pub(crate) battery_hist: Rc<RefCell<Vec<f32>>>,
    pub(crate) gpu_gauges: RefCell<Vec<views::TempGaugeCard>>,
    pub(crate) open: Cell<bool>,
    pub(crate) pulling: Cell<bool>,
    pub(crate) animating: Cell<bool>,
    pub(crate) current_extent: Cell<i32>,
    pub(crate) max_extent: Cell<i32>,
    pub(crate) snapshot: RefCell<DashboardSnapshot>,
    pub(crate) text_filter: RefCell<String>,
    pub(crate) class_filter: RefCell<ProcessClassFilter>,
    pub(crate) sort_column: RefCell<ProcessSortColumn>,
    pub(crate) sort_direction: RefCell<SortDirection>,
    /// PIDs whose children are shown in the Processes tree.
    pub(crate) expanded_processes: RefCell<HashSet<u32>>,
    pub(crate) last_legend_cores: Cell<usize>,
    pub(crate) last_disk_sig: RefCell<String>,
    pub(crate) last_relayout_key: Cell<(i32, i32)>,
    pub(crate) last_process_sig: RefCell<u64>,
}

#[allow(unused_imports)] // re-exports for crate-wide `ui::dashboard::…` callers
pub use lifecycle::{
    attach_poll_channel, init, is_open, on_bar_config_changed, on_dashboard_config_changed,
    on_theme_changed, reload_for_locale, request_close, request_toggle, set_below_screenshot,
    wire_bar_pull,
};

mod dropdown {
    pub fn request_close_all() {
        crate::ui::bar::close_popovers();
    }
}
