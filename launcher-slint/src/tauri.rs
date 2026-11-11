//! The little of Tauri that the Tauri launcher's command modules use (chat.rs, settings.rs and
//! the others, carried over as they are): the app's states, its events and its window, here
//! answered by the Slint window (ui.rs). So the launcher keeps one command layer while the Tauri
//! one is retired, and those modules read as they did.

use std::any::Any;
use std::any::TypeId;
use std::collections::HashMap;
use std::sync::Arc;

/// What an event tells the window (its name, as the Tauri page listened for it).
pub type EventSink = Arc<dyn Fn(&str) + Send + Sync>;
/// The window's attention and focus, for chat's mentions.
pub type WindowHooks = Arc<dyn Fn(WindowCall) -> bool + Send + Sync>;

pub enum WindowCall {
    IsFocused,
    RequestAttention,
    /// Text onto the clipboard (Copy details, an identity to move).
    Copy(String),
    /// The player's size for the launcher (1.0 as designed), times the system's own scaling.
    Size(f32),
    FullScreen,
    /// The look, chosen in Settings › Display.
    Theme(launcher::prefs::Theme),
    /// Another screen ("play", "settings:network"…).
    Go(String),
    /// The account dialog: a name for a new account on `server`.
    Account {
        server: String,
        suggested: String,
        taken: bool,
        own: bool,
    },
    /// The feedback dialog (Settings › Feedback's Send feedback, or after a game).
    Feedback,
    /// This release's What's new (Settings › About, as the version in the side menu does).
    WhatsNew,
    /// Close the launcher now (Close now, send it next time).
    Quit,
}

struct Inner {
    states: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    events: EventSink,
    window: WindowHooks,
}

/// The app: its states (one of each type), its events and its window.
#[derive(Clone)]
pub struct AppHandle(Arc<Inner>);

/// Builds the app, a state at a time, as `tauri::Builder::manage` did.
pub struct Builder {
    states: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Builder {
    pub fn new() -> Self {
        Self { states: HashMap::new() }
    }

    pub fn manage<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.states.insert(TypeId::of::<T>(), Box::new(state));
        self
    }

    pub fn build(self, events: EventSink, window: WindowHooks) -> AppHandle {
        AppHandle(Arc::new(Inner {
            states: self.states,
            events,
            window,
        }))
    }
}

/// A state of the app, borrowed: what `tauri::State` was.
pub struct State<'a, T: 'static>(&'a T);

impl<T> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<'_, T> {}

impl<T> std::ops::Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

pub trait Manager {
    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T>;
    fn get_webview_window(&self, label: &str) -> Option<Window>;
}

impl Manager for AppHandle {
    /// The app's state of type `T`. Every one the modules ask for is managed at start (ui.rs):
    /// a missing one is a mistake in the launcher, not something a player can cause.
    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        let state = self.0.states.get(&TypeId::of::<T>()).and_then(|s| s.downcast_ref::<T>());
        State(state.unwrap_or_else(|| panic!("{} isn't managed", std::any::type_name::<T>())))
    }

    fn get_webview_window(&self, _label: &str) -> Option<Window> {
        Some(Window(self.0.window.clone()))
    }
}

impl AppHandle {
    /// Asks the window for something (not one of Tauri's: the Slint pages' own way to it).
    pub fn window(&self, call: WindowCall) -> bool {
        (self.0.window)(call)
    }
}

pub trait Emitter {
    fn emit<S>(&self, event: &str, payload: S) -> Result<(), String>;
}

impl Emitter for AppHandle {
    /// Tells the window: the page asked again for what it showed, and so does the window.
    fn emit<S>(&self, event: &str, _payload: S) -> Result<(), String> {
        (self.0.events)(event);
        Ok(())
    }
}

/// The launcher's window, for what chat asks of it.
pub struct Window(WindowHooks);

pub enum UserAttentionType {
    Informational,
}

impl Window {
    pub fn is_focused(&self) -> Result<bool, String> {
        Ok((self.0)(WindowCall::IsFocused))
    }

    pub fn request_user_attention(&self, _kind: Option<UserAttentionType>) -> Result<(), String> {
        (self.0)(WindowCall::RequestAttention);
        Ok(())
    }
}
