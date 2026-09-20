//! Sidebar / category structure — single source of truth for Settings v2 nav.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

/// Accent hue for icon badges (home tiles, mini sidebar, page headers).
#[derive(Clone, Copy)]
pub enum NavHue {
    Blue,
    Purple,
    Pink,
    Orange,
    Teal,
    Green,
    Gray,
    #[allow(dead_code)] // CSS class reserved for future nav accents
    Yellow,
}

impl NavHue {
    pub fn css_class(self) -> &'static str {
        match self {
            Self::Blue => "metis-nav-hue-blue",
            Self::Purple => "metis-nav-hue-purple",
            Self::Pink => "metis-nav-hue-pink",
            Self::Orange => "metis-nav-hue-orange",
            Self::Teal => "metis-nav-hue-teal",
            Self::Green => "metis-nav-hue-green",
            Self::Gray => "metis-nav-hue-gray",
            Self::Yellow => "metis-nav-hue-yellow",
        }
    }
}

/// Top-level Settings category (Home tile + mini-sidebar entry).
#[derive(Clone, Copy)]
pub struct Category {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    pub hue: NavHue,
    /// Short blurb on the Home tile.
    pub blurb: &'static str,
}

pub const CATEGORIES: &[Category] = &[
    Category {
        id: "displays",
        title: "Displays",
        icon: "video-display-symbolic",
        hue: NavHue::Blue,
        blurb: "Arrangement, resolution, scale, and night light",
    },
    Category {
        id: "desktop",
        title: "Desktop",
        icon: "preferences-desktop-wallpaper-symbolic",
        hue: NavHue::Purple,
        blurb: "Appearance, wallpaper, edge bar, windows, and briefing",
    },
    Category {
        id: "connectivity",
        title: "Connectivity",
        icon: "network-wireless-symbolic",
        hue: NavHue::Blue,
        blurb: "Wi-Fi, Ethernet, VPN, and Bluetooth",
    },
    Category {
        id: "input",
        title: "Input",
        icon: "input-keyboard-symbolic",
        hue: NavHue::Gray,
        blurb: "Mouse, touchpad, keyboard, and shortcuts",
    },
    Category {
        id: "system",
        title: "System",
        icon: "preferences-system-symbolic",
        hue: NavHue::Green,
        blurb: "Users, date & time, sound, power, locale, remote access, and more",
    },
];

pub struct NavItem {
    pub page_id: Option<&'static str>,
    pub title: &'static str,
    pub icon: Option<&'static str>,
    pub hue: Option<NavHue>,
    /// Shown under the page title in the content area.
    pub subtitle: Option<&'static str>,
    /// Category id this page belongs to (`None` for section headers).
    pub category: Option<&'static str>,
}

pub const NAV: &[NavItem] = &[
    NavItem {
        page_id: None,
        title: "Displays",
        icon: None,
        hue: None,
        subtitle: None,
        category: None,
    },
    NavItem {
        page_id: Some("display"),
        title: "Display",
        icon: Some("video-display-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Graphics profile, arrangement, resolution, scale, and night light"),
        category: Some("displays"),
    },
    NavItem {
        page_id: None,
        title: "Desktop",
        icon: None,
        hue: None,
        subtitle: None,
        category: None,
    },
    NavItem {
        page_id: Some("appearance"),
        title: "Appearance",
        icon: Some("preferences-desktop-appearance-symbolic"),
        hue: Some(NavHue::Pink),
        subtitle: Some("Theme mode, accent colours, and interface font"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("background"),
        title: "Background",
        icon: Some("preferences-desktop-wallpaper-symbolic"),
        hue: Some(NavHue::Purple),
        subtitle: Some("Desktop wallpaper: picture, solid colour, or gradient"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("edgebar"),
        title: "Edge bar",
        icon: Some("preferences-system-symbolic"),
        hue: Some(NavHue::Teal),
        subtitle: Some("Position, opacity, blur, workspaces, and the bar border"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("desktop_widgets"),
        title: "Desktop widgets",
        icon: Some("view-grid-symbolic"),
        hue: Some(NavHue::Purple),
        subtitle: Some("Optional wallpaper widgets: folders, apps, clock, and more"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("windows"),
        title: "Windows",
        icon: Some("window-new-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Animations, titlebar opacity, and window borders"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("titlebars"),
        title: "App titlebars",
        icon: Some("window-new-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Override Metis vs app titlebars per application"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("menu"),
        title: "Metis Menu",
        icon: Some("view-app-grid-symbolic"),
        hue: Some(NavHue::Purple),
        subtitle: Some("Launcher apps and menu panel look"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("weather"),
        title: "Weather",
        icon: Some("weather-few-clouds-symbolic"),
        hue: Some(NavHue::Teal),
        subtitle: Some("Briefing weather card on the edge bar"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: Some("calendars"),
        title: "Calendars",
        icon: Some("x-office-calendar-symbolic"),
        hue: Some(NavHue::Orange),
        subtitle: Some("Calendar accounts for the briefing"),
        category: Some("desktop"),
    },
    NavItem {
        page_id: None,
        title: "Connectivity",
        icon: None,
        hue: None,
        subtitle: None,
        category: None,
    },
    NavItem {
        page_id: Some("network"),
        title: "Network",
        icon: Some("network-wireless-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Wi-Fi, Ethernet, DNS, VPN, and proxy"),
        category: Some("connectivity"),
    },
    NavItem {
        page_id: Some("bluetooth"),
        title: "Bluetooth",
        icon: Some("bluetooth-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Pair and manage Bluetooth devices"),
        category: Some("connectivity"),
    },
    NavItem {
        page_id: None,
        title: "Input",
        icon: None,
        hue: None,
        subtitle: None,
        category: None,
    },
    NavItem {
        page_id: Some("mouse"),
        title: "Mouse",
        icon: Some("input-mouse-symbolic"),
        hue: Some(NavHue::Gray),
        subtitle: Some("Pointer speed, acceleration, and scrolling"),
        category: Some("input"),
    },
    NavItem {
        page_id: Some("touchpad"),
        title: "Touchpad",
        icon: Some("input-touchpad-symbolic"),
        hue: Some(NavHue::Gray),
        subtitle: Some("Gestures, tap-to-click, and natural scroll"),
        category: Some("input"),
    },
    NavItem {
        page_id: Some("keyboard"),
        title: "Keyboard",
        icon: Some("input-keyboard-symbolic"),
        hue: Some(NavHue::Gray),
        subtitle: Some("Repeat rate and layout preferences"),
        category: Some("input"),
    },
    NavItem {
        page_id: Some("shortcuts"),
        title: "Shortcuts",
        icon: Some("preferences-desktop-keyboard-shortcuts-symbolic"),
        hue: Some(NavHue::Gray),
        subtitle: Some("Search and browse current desktop shortcuts"),
        category: Some("input"),
    },
    NavItem {
        page_id: None,
        title: "System",
        icon: None,
        hue: None,
        subtitle: None,
        category: None,
    },
    NavItem {
        page_id: Some("users"),
        title: "Users",
        icon: Some("system-users-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Profile, password, and local accounts"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("date_time"),
        title: "Date & Time",
        icon: Some("preferences-system-time-symbolic"),
        hue: Some(NavHue::Teal),
        subtitle: Some("Clock, time zone, and calendar week start"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("locale"),
        title: "Language & region",
        icon: Some("preferences-desktop-locale-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Session language and number/date formats"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("control_center"),
        title: "Control Center",
        icon: Some("utilities-system-monitor-symbolic"),
        hue: Some(NavHue::Green),
        subtitle: Some("System monitor panel, refresh rate, and process controls"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("sound"),
        title: "Sound",
        icon: Some("audio-volume-high-symbolic"),
        hue: Some(NavHue::Pink),
        subtitle: Some("Output and input audio devices"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("power"),
        title: "Power",
        icon: Some("battery-level-100-symbolic"),
        hue: Some(NavHue::Green),
        subtitle: Some("Battery, profiles, and idle behaviour"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("startup"),
        title: "Startup",
        icon: Some("system-run-symbolic"),
        hue: Some(NavHue::Gray),
        subtitle: Some("Applications that launch after you sign in"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("remote"),
        title: "Remote access",
        icon: Some("network-transmit-receive-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Share your logged-in session over the network"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("gaming"),
        title: "Gaming",
        icon: Some("applications-games-symbolic"),
        hue: Some(NavHue::Orange),
        subtitle: Some("Gamepads, touchscreens, Steam, and GPU hints"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("printers"),
        title: "Printers",
        icon: Some("printer-symbolic"),
        hue: Some(NavHue::Gray),
        subtitle: Some("Installed printers and system print settings"),
        category: Some("system"),
    },
    NavItem {
        page_id: Some("screenshot"),
        title: "Screenshot",
        icon: Some("camera-photo-symbolic"),
        hue: Some(NavHue::Blue),
        subtitle: Some("Capture defaults, editor, and save location"),
        category: Some("system"),
    },
];

pub fn page_ids() -> Vec<&'static str> {
    NAV.iter().filter_map(|item| item.page_id).collect()
}

pub fn category_by_id(id: &str) -> Option<&'static Category> {
    CATEGORIES.iter().find(|c| c.id == id)
}

pub fn category_for_page(page_id: &str) -> Option<&'static Category> {
    let cat_id = NAV
        .iter()
        .find(|item| item.page_id == Some(page_id))?
        .category?;
    category_by_id(cat_id)
}

/// Pages belonging to a category, in nav order.
pub fn pages_in_category(category_id: &str) -> Vec<&'static NavItem> {
    NAV.iter()
        .filter(|item| item.page_id.is_some() && item.category == Some(category_id))
        .collect()
}

use crate::gtk_cb::OptFnStrRef;

thread_local! {
    static PAGE_REQUEST: OptFnStrRef = const { RefCell::new(None) };
}

/// Register a callback used by pages that need to jump to another Settings page.
pub fn set_page_request_handler(handler: Rc<dyn Fn(&str)>) {
    PAGE_REQUEST.with(|slot| {
        *slot.borrow_mut() = Some(handler);
    });
}

/// Navigate to another Settings page (e.g. Shortcuts → Keyboard).
pub fn request_page(page_id: &str) {
    PAGE_REQUEST.with(|slot| {
        if let Some(handler) = slot.borrow().as_ref() {
            handler(page_id);
        }
    });
}

pub fn meta_for(page_id: &str) -> Option<&'static NavItem> {
    NAV.iter().find(|item| item.page_id == Some(page_id))
}

fn lowercase_titles() -> &'static [String] {
    static TITLES: OnceLock<Vec<String>> = OnceLock::new();
    TITLES.get_or_init(|| {
        NAV.iter()
            .map(|item| {
                if item.page_id.is_some() {
                    item.title.to_ascii_lowercase()
                } else {
                    String::new()
                }
            })
            .collect()
    })
}

/// Whether a nav row at `index` should stay visible for `query` (legacy full sidebar).
#[allow(dead_code)]
pub fn row_visible_for_search(index: usize, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let Some(item) = NAV.get(index) else {
        return false;
    };
    let titles = lowercase_titles();
    if item.page_id.is_some() {
        return titles[index].contains(query);
    }
    section_visible_for_search(index, query, titles)
}

#[allow(dead_code)]
fn section_visible_for_search(section_index: usize, query: &str, titles: &[String]) -> bool {
    for (index, item) in NAV.iter().enumerate().skip(section_index + 1) {
        if item.page_id.is_none() {
            break;
        }
        if titles[index].contains(query) {
            return true;
        }
    }
    false
}

/// Page ids whose title matches `query` (lowercase).
pub fn matching_page_ids(query: &str) -> Vec<&'static str> {
    if query.is_empty() {
        return page_ids();
    }
    let q = query.to_ascii_lowercase();
    NAV.iter()
        .filter_map(|item| {
            let id = item.page_id?;
            if item.title.to_ascii_lowercase().contains(&q)
                || item
                    .subtitle
                    .is_some_and(|s| s.to_ascii_lowercase().contains(&q))
            {
                Some(id)
            } else {
                None
            }
        })
        .collect()
}

/// Category whose title/blurb matches, or that contains a matching page.
pub fn category_matches(cat: &Category, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_ascii_lowercase();
    if cat.title.to_ascii_lowercase().contains(&q) || cat.blurb.to_ascii_lowercase().contains(&q) {
        return true;
    }
    pages_in_category(cat.id)
        .iter()
        .any(|p| p.title.to_ascii_lowercase().contains(&q))
}
