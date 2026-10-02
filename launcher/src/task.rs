//! Work that must not block the window (network calls, file scans, installs)
//! runs on a thread. The UI polls its result each frame.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::Mutex;

thread_local! {
    /// Set on a task's thread: a panic there ends the task, not the launcher
    /// (see `logging`).
    static IN_TASK: Cell<bool> = const { Cell::new(false) };
}

/// Whether this thread runs a [`Task`].
pub fn in_task() -> bool {
    IN_TASK.with(Cell::get)
}

/// What a task gives when it panicked instead of returning: an error where
/// it returns one, so the screen waiting on it shows that and stops waiting.
pub trait FromPanic {
    fn from_panic(message: String) -> Self;
}

impl<T> FromPanic for Result<T, String> {
    fn from_panic(message: String) -> Self {
        Err(message)
    }
}

impl<T> FromPanic for anyhow::Result<T> {
    fn from_panic(message: String) -> Self {
        Err(anyhow::anyhow!(message))
    }
}

/// Lists (found game folders, test results): nothing found.
impl<T> FromPanic for Vec<T> {
    fn from_panic(_message: String) -> Self {
        Vec::new()
    }
}

/// The checklist's facts: none, and the reason where the game version goes.
impl FromPanic for (setup::diagnose::Facts, crate::flow::Support) {
    fn from_panic(message: String) -> Self {
        (setup::diagnose::Facts::default(), crate::flow::Support::Unknown(message))
    }
}

/// A result that arrives later.
pub struct Task<T> {
    result: Arc<Mutex<Option<T>>>,
    done: bool,
}

impl<T: FromPanic + Send + 'static> Task<T> {
    /// Runs `f` on a new thread and repaints `ctx` when it's done, also when
    /// it panicked (its result is then [`FromPanic`]'s).
    pub fn spawn(ctx: &eframe::egui::Context, f: impl FnOnce() -> T + Send + 'static) -> Self {
        let result = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&result);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            IN_TASK.with(|t| t.set(true));
            let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|payload| {
                let message = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown".into());
                T::from_panic(format!("Something went wrong inside the launcher: {message}"))
            });
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

impl<T: FromPanic + Send + 'static> Slot<T> {
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
