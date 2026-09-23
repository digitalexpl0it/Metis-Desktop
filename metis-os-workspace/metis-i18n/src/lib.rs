//! Metis localization helpers.
//!
//! - **GTK apps** (`metis-shell`, `metis-settings`): GNU gettext via [`tr`] / [`trn`].
//! - **Compositor**: Fluent via [`tr_ftl`].
//! - Locale resolve from `locale.json` then `LANG` / `LC_*`.
//! - Locale-aware date/number formatting (chrono localized patterns).

mod format;
mod paths;

use std::collections::HashMap;
use std::path::PathBuf;

use fluent::concurrent::FluentBundle;
use fluent::{FluentArgs, FluentResource, FluentValue};
use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, ngettext, setlocale,
    textdomain,
};
use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use unic_langid::LanguageIdentifier;

pub use format::{
    format_decimal, format_pattern, format_short_date, format_short_datetime, format_short_time,
};
pub use paths::{GETTEXT_DOMAIN, catalog_roots, discover_installed_languages};

static STATE: OnceCell<RwLock<I18nState>> = OnceCell::new();

/// Snapshot of the active locale.
#[derive(Debug, Clone)]
pub struct LocaleInfo {
    /// Canonical tag used for catalogs (`en`, `en_US`, `ar`, …).
    pub tag: String,
    /// gettext / POSIX style (`en_US`).
    pub posix: String,
    pub is_rtl: bool,
    pub formats_from_locale: bool,
}

struct I18nState {
    info: LocaleInfo,
    fluent: FluentRuntime,
}

/// Fluent bundle pair (active locale + English fallback) for compositor strings.
pub struct FluentRuntime {
    primary: FluentBundle<FluentResource>,
    fallback: FluentBundle<FluentResource>,
}

impl FluentRuntime {
    fn load(tag: &str) -> Self {
        let fallback = load_fluent_bundle("en");
        let lang_only = tag.split(['_', '-']).next().unwrap_or(tag);
        let mut chosen = None;
        for candidate in [tag, lang_only] {
            if let Some(bundle) = try_load_fluent_bundle(candidate) {
                chosen = Some(bundle);
                break;
            }
        }
        let primary = chosen.unwrap_or_else(|| load_fluent_bundle("en"));
        Self { primary, fallback }
    }

    pub fn tr(&self, id: &str) -> String {
        self.tr_args(id, None)
    }

    pub fn tr_args(&self, id: &str, args: Option<&FluentArgs>) -> String {
        for bundle in [&self.primary, &self.fallback] {
            if let Some(msg) = bundle.get_message(id)
                && let Some(pattern) = msg.value()
            {
                let mut errors = Vec::new();
                let value = bundle.format_pattern(pattern, args, &mut errors);
                if !errors.is_empty() {
                    tracing::debug!(id, ?errors, "fluent format errors");
                }
                return value.into_owned();
            }
        }
        tracing::debug!(id, "fluent missing key; returning id");
        id.to_string()
    }
}

fn try_load_fluent_bundle(tag: &str) -> Option<FluentBundle<FluentResource>> {
    for root in catalog_roots() {
        let path = root.join(tag).join("compositor").join("metis.ftl");
        if let Ok(source) = std::fs::read_to_string(&path) {
            let lang: LanguageIdentifier = tag
                .replace('_', "-")
                .parse()
                .unwrap_or_else(|_| "en".parse().unwrap());
            let mut bundle = FluentBundle::new_concurrent(vec![lang]);
            match FluentResource::try_new(source) {
                Ok(res) => {
                    if let Err(errs) = bundle.add_resource(res) {
                        tracing::warn!(?errs, path = %path.display(), "fluent add_resource");
                    }
                    tracing::info!(path = %path.display(), "loaded fluent catalog");
                    return Some(bundle);
                }
                Err((res, errs)) => {
                    tracing::warn!(?errs, path = %path.display(), "fluent parse");
                    let _ = bundle.add_resource(res);
                    return Some(bundle);
                }
            }
        }
    }
    None
}

fn load_fluent_bundle(tag: &str) -> FluentBundle<FluentResource> {
    try_load_fluent_bundle(tag).unwrap_or_else(|| {
        let lang: LanguageIdentifier = tag
            .replace('_', "-")
            .parse()
            .unwrap_or_else(|_| "en".parse().unwrap());
        FluentBundle::new_concurrent(vec![lang])
    })
}

/// Initialize gettext + Fluent from `locale.json` / environment.
/// Safe to call multiple times (rebinds on subsequent calls).
pub fn init() {
    let info = resolve_locale();
    apply_gettext(&info);
    let fluent = FluentRuntime::load(&info.tag);
    let state = I18nState { info, fluent };
    if let Some(lock) = STATE.get() {
        *lock.write() = state;
    } else {
        let _ = STATE.set(RwLock::new(state));
    }
}

/// Re-read `locale.json` and rebind catalogs (live language switch).
pub fn reload() {
    init();
}

pub fn locale_info() -> LocaleInfo {
    ensure_state();
    STATE.get().unwrap().read().info.clone()
}

pub fn is_rtl() -> bool {
    locale_info().is_rtl
}

/// gettext lookup (msgid is the English source string).
pub fn tr(msg: &str) -> String {
    ensure_state();
    gettext(msg)
}

/// gettext plural lookup.
pub fn trn(singular: &str, plural: &str, n: u64) -> String {
    ensure_state();
    ngettext(singular, plural, n.try_into().unwrap_or(u32::MAX))
}

/// Fluent lookup for compositor UI strings.
pub fn tr_ftl(id: &str) -> String {
    ensure_state();
    STATE.get().unwrap().read().fluent.tr(id)
}

/// Fluent lookup with a single string argument `{ $name }`.
pub fn tr_ftl_arg(id: &str, name: &str, value: &str) -> String {
    ensure_state();
    let mut args = FluentArgs::new();
    args.set(name, FluentValue::from(value));
    STATE.get().unwrap().read().fluent.tr_args(id, Some(&args))
}

fn ensure_state() {
    if STATE.get().is_none() {
        init();
    }
}

fn apply_gettext(info: &LocaleInfo) {
    let lang = info
        .tag
        .split(['_', '-'])
        .next()
        .unwrap_or(info.tag.as_str());
    // GNU gettext ignores LANGUAGE when the active locale is C/POSIX. Prefer a
    // real UTF-8 locale (even en_US) and select catalogs via LANGUAGE so
    // Spanish/etc. work without `locale-gen es_ES.UTF-8` on the host.
    let language_chain = if lang.eq_ignore_ascii_case("en") {
        "en".to_string()
    } else {
        format!("{}:{}:en", info.posix, lang)
    };
    // SAFETY: locale init/reload runs on the UI thread before widgets re-read env.
    unsafe {
        std::env::set_var("LANGUAGE", &language_chain);
    }

    let mut locale_candidates = vec![
        format!("{}.UTF-8", info.posix),
        format!("{}.utf8", info.posix),
        info.posix.clone(),
    ];
    if !info.posix.eq_ignore_ascii_case("en_US") {
        locale_candidates.push("en_US.UTF-8".into());
        locale_candidates.push("en_US.utf8".into());
    }
    // Last resort only — LANGUAGE is ignored under C, so translations break.
    locale_candidates.push("C.UTF-8".into());

    let mut applied = None;
    for cand in &locale_candidates {
        // SAFETY: same invariant as the `set_var` above — only the UI thread
        // touches process locale, and it does so before widgets re-read it.
        if unsafe { setlocale(LocaleCategory::LcAll, cand.as_str()) }.is_some() {
            applied = Some(cand.clone());
            break;
        }
    }
    match applied.as_deref() {
        Some(loc) if loc.starts_with('C') || loc.eq_ignore_ascii_case("POSIX") => {
            tracing::warn!(
                wanted = %info.posix,
                "setlocale fell back to C; gettext will not honour LANGUAGE — install a UTF-8 locale (e.g. en_US.UTF-8)"
            );
        }
        Some(loc) => {
            tracing::info!(locale = %loc, language = %language_chain, "gettext locale bound")
        }
        None => tracing::warn!(wanted = %info.posix, "setlocale failed for all candidates"),
    }

    for root in catalog_roots() {
        if root.is_dir() {
            match bindtextdomain(GETTEXT_DOMAIN, root.to_string_lossy().as_ref()) {
                Ok(bound) => {
                    tracing::info!(dir = %bound.display(), "gettext bindtextdomain");
                    break;
                }
                Err(err) => tracing::warn!(?err, dir = %root.display(), "bindtextdomain"),
            }
        }
    }
    if let Err(err) = textdomain(GETTEXT_DOMAIN) {
        tracing::warn!(?err, "textdomain");
    }
    if let Err(err) = bind_textdomain_codeset(GETTEXT_DOMAIN, "UTF-8") {
        tracing::debug!(?err, "bind_textdomain_codeset");
    }
}

/// Resolve active locale from config then environment.
pub fn resolve_locale() -> LocaleInfo {
    let cfg = metis_config::load_locale_config();
    let raw = cfg
        .locale
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(env_locale)
        .unwrap_or_else(|| "en_US".into());

    let tag = normalize_tag(&raw);
    let posix = to_posix(&tag);
    let is_rtl = language_is_rtl(&tag);
    LocaleInfo {
        tag,
        posix,
        is_rtl,
        formats_from_locale: cfg.formats_from_locale,
    }
}

fn env_locale() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim();
            if !v.is_empty() && v != "C" && !v.eq_ignore_ascii_case("POSIX") {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn normalize_tag(raw: &str) -> String {
    let base = raw.split(['.', '@']).next().unwrap_or(raw);
    let base = base.replace('-', "_");
    if base.is_empty() { "en".into() } else { base }
}

fn to_posix(tag: &str) -> String {
    if !tag.contains('_') {
        match tag {
            "en" => "en_US".into(),
            "es" => "es_ES".into(),
            "fr" => "fr_FR".into(),
            "de" => "de_DE".into(),
            "ar" => "ar_SA".into(),
            "he" => "he_IL".into(),
            "ja" => "ja_JP".into(),
            "zh" => "zh_CN".into(),
            other => format!("{other}_{}", other.to_uppercase()),
        }
    } else {
        tag.to_string()
    }
}

fn language_is_rtl(tag: &str) -> bool {
    let lang = tag.split('_').next().unwrap_or(tag).to_ascii_lowercase();
    matches!(
        lang.as_str(),
        "ar" | "he" | "fa" | "ur" | "ps" | "sd" | "yi" | "dv"
    )
}

/// Human labels for language pickers: `(tag, display name)`. Empty tag = system default.
pub fn known_language_choices() -> Vec<(String, String)> {
    let mut out = vec![
        (String::new(), "System default".into()),
        ("en".into(), "English".into()),
        ("es".into(), "Spanish".into()),
        ("fr".into(), "French".into()),
        ("de".into(), "German".into()),
        ("pt".into(), "Portuguese".into()),
        ("it".into(), "Italian".into()),
        ("nl".into(), "Dutch".into()),
        ("pl".into(), "Polish".into()),
        ("ru".into(), "Russian".into()),
        ("ja".into(), "Japanese".into()),
        ("zh".into(), "Chinese".into()),
        ("ko".into(), "Korean".into()),
        ("ar".into(), "Arabic".into()),
        ("he".into(), "Hebrew".into()),
    ];
    let known: HashMap<String, ()> = out
        .iter()
        .filter(|(t, _)| !t.is_empty())
        .map(|(t, _)| (t.clone(), ()))
        .collect();
    for tag in discover_installed_languages() {
        let lang = tag.split('_').next().unwrap_or(&tag).to_string();
        if !known.contains_key(&lang) && !known.contains_key(&tag) {
            out.push((lang.clone(), lang));
        }
    }
    out
}

/// Path helpers used by install/tests.
pub fn fluent_catalog_path(tag: &str) -> Option<PathBuf> {
    for root in catalog_roots() {
        let p = root.join(tag).join("compositor").join("metis.ftl");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn share_locale_dir() -> PathBuf {
    paths::primary_share_locale()
}
