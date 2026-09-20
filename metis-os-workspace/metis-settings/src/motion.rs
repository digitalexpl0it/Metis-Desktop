//! Animation helpers that honour `gtk-enable-animations`.

/// Transition duration in milliseconds, or `0` when animations are disabled.
pub fn ms(preferred: u32) -> u32 {
    if gtk::Settings::default().is_some_and(|s| s.is_gtk_enable_animations()) {
        preferred
    } else {
        0
    }
}
