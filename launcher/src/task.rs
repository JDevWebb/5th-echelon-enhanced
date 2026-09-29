//! Work that must not block the window (network calls, file scans, installs)
//! runs on a thread. The UI polls its result each frame.

use std::sync::Arc;
use std::sync::Mutex;

/// A result that arrives later.
pub struct Task<T> {
    result: Arc<Mutex<Option<T>>>,
    done: bool,
}

impl<T: Send + 'static> Task<T> {
    /// Runs `f` on a new thread and repaints `ctx` when it's done.
    pub fn spawn(ctx: &eframe::egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let result = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&result);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let value = f();
            *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
            ctx.request_repaint();
        });
        Self { result, done: false }
    }

    /// The result, once, when it has arrived.
    pub fn take(&mut self) -> Option<T> {
        if self.done {
            return None;
        }
        let value = self.result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        self.done = value.is_some();
        value
    }
}

/// An optional running task: `poll` hands its result over once.
pub struct Slot<T>(Option<Task<T>>);

impl<T: Send + 'static> Default for Slot<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Send + 'static> Slot<T> {
    pub fn start(&mut self, ctx: &eframe::egui::Context, f: impl FnOnce() -> T + Send + 'static) {
        self.0 = Some(Task::spawn(ctx, f));
    }

    pub fn running(&self) -> bool {
        self.0.is_some()
    }

    pub fn poll(&mut self) -> Option<T> {
        let value = self.0.as_mut()?.take();
        if value.is_some() {
            self.0 = None;
        }
        value
    }
}
