//! Run blocking Polkit/`metis-remote` work off the GTK thread, then invoke a
//! main-thread callback with the result (widgets stay on the UI thread).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use gtk::glib;

/// Spawn `work` on a background thread; when it finishes, run `on_done` on the
/// GLib main context (so GTK widgets are safe).
pub fn run_bg<T, W, D>(work: W, on_done: D)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(T) + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let rx = Rc::new(RefCell::new(rx));
    let on_done = Rc::new(RefCell::new(Some(on_done)));
    glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        match rx.borrow().try_recv() {
            Ok(value) => {
                if let Some(cb) = on_done.borrow_mut().take() {
                    cb(value);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}
