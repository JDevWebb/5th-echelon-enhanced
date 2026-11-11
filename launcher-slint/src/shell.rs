//! The window's other screens and its dialogs (pages/), the app the command modules live in
//! (tauri.rs's stand-in: their states, their events, what they ask of the window), and the
//! update banner. Play and Matches are ui.rs's.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use slint::ComponentHandle;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;

use crate::pages::dialogs::Dialog;
use crate::pages::PageView;
use crate::pages::Screen;
use crate::pages::R;
use crate::tauri::AppHandle;
use crate::tauri::Manager;
use crate::tauri::WindowCall;
use crate::ui::Ui;
use crate::AppWindow;

/// The pages, each kept between visits (what was typed, the section open).
pub struct Pages {
    pub settings: Mutex<crate::pages::settings::SettingsPage>,
    pub friends: Mutex<crate::pages::friends::FriendsPage>,
    pub servers: Mutex<crate::pages::servers::ServersPage>,
    pub host: Mutex<crate::pages::host::HostPage>,
    pub intel: Mutex<crate::pages::intel::IntelPage>,
    pub support: Mutex<crate::pages::support::SupportPage>,
    pub chat: Mutex<crate::pages::chat::ChatPage>,
    pub setup: Mutex<crate::pages::setup::SetupPage>,
    pub dialog: Mutex<Option<Dialog>>,
    /// Dialogs waiting their turn (one shows at a time).
    pub queue: Mutex<Vec<Dialog>>,
    pub refreshed: Mutex<Instant>,
    pub focused: AtomicBool,
}

impl Pages {
    pub fn new(fresh: bool) -> Self {
        let setup = crate::pages::setup::SetupPage::new(fresh);
        Self {
            settings: Mutex::new(crate::pages::settings::SettingsPage::new()),
            friends: Mutex::default(),
            servers: Mutex::default(),
            host: Mutex::default(),
            intel: Mutex::default(),
            support: Mutex::default(),
            chat: Mutex::new(crate::pages::chat::ChatPage::new()),
            setup: Mutex::new(setup),
            dialog: Mutex::new(None),
            queue: Mutex::new(Vec::new()),
            refreshed: Mutex::new(Instant::now()),
            focused: AtomicBool::new(true),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

/// A row for the window (built in its thread: a row holds models).
pub(crate) fn row(r: &R) -> crate::Row {
    crate::Row {
        kind: s(r.kind),
        id: s(&r.id),
        label: s(&r.label),
        text: s(&r.text),
        sub: s(&r.sub),
        tone: s(r.tone),
        on: r.on,
        enabled: r.enabled,
        options: ModelRc::new(VecModel::from(r.options.iter().map(s).collect::<Vec<_>>())),
        current: r.current,
        value: r.value,
        min: r.min,
        max: r.max,
        placeholder: s(&r.placeholder),
        secret: r.secret,
        buttons: ModelRc::new(VecModel::from(
            r.buttons
                .iter()
                .map(|b| crate::RowButton {
                    id: s(&b.id),
                    label: s(&b.label),
                    kind: s(b.kind),
                    enabled: b.enabled,
                })
                .collect::<Vec<_>>(),
        )),
        edge: r.edge,
    }
}

/// `rows` into the model shown, row by row: a row that didn't change isn't touched, so a field
/// being typed in keeps its text, its focus and the page its scroll.
fn set_rows(current: &ModelRc<crate::Row>, rows: &[R], replace: impl FnOnce(ModelRc<crate::Row>)) {
    if let Some(vec) = current.as_any().downcast_ref::<VecModel<crate::Row>>() {
        let shown = vec.row_count();
        for (i, r) in rows.iter().enumerate() {
            let new = row(r);
            if i < shown {
                let same = vec.row_data(i).is_some_and(|old| same_row(&old, &new));
                if !same {
                    vec.set_row_data(i, new);
                }
            } else {
                vec.push(new);
            }
        }
        while vec.row_count() > rows.len() {
            vec.remove(vec.row_count() - 1);
        }
        return;
    }
    replace(ModelRc::new(VecModel::from(rows.iter().map(row).collect::<Vec<_>>())));
}

/// Two rows alike (their lists by content: the models compare by identity).
fn same_row(a: &crate::Row, b: &crate::Row) -> bool {
    let list = |m: &ModelRc<SharedString>| m.iter().collect::<Vec<_>>();
    let buttons = |m: &ModelRc<crate::RowButton>| m.iter().collect::<Vec<_>>();
    a.kind == b.kind
        && a.id == b.id
        && a.label == b.label
        && a.text == b.text
        && a.sub == b.sub
        && a.tone == b.tone
        && a.on == b.on
        && a.enabled == b.enabled
        && a.current == b.current
        && a.value == b.value
        && a.min == b.min
        && a.max == b.max
        && a.placeholder == b.placeholder
        && a.secret == b.secret
        && a.edge == b.edge
        && list(&a.options) == list(&b.options)
        && buttons(&a.buttons) == buttons(&b.buttons)
}

impl Ui {
    /// Runs `f` on the page called `screen`, if it's one of the pages.
    pub fn with_page<T>(&self, screen: &str, f: impl FnOnce(&mut dyn Screen) -> T) -> Option<T> {
        let p = &self.pages;
        Some(match screen {
            "settings" => f(&mut *lock(&p.settings)),
            "friends" => f(&mut *lock(&p.friends)),
            "servers" => f(&mut *lock(&p.servers)),
            "host" => f(&mut *lock(&p.host)),
            "intel" => f(&mut *lock(&p.intel)),
            "support" => f(&mut *lock(&p.support)),
            "chat" => f(&mut *lock(&p.chat)),
            "setup" => f(&mut *lock(&p.setup)),
            _ => return None,
        })
    }

    /// The page shown, built again (on a worker thread) and handed to the window.
    pub fn load_page(self: &Arc<Self>) {
        if matches!(self.screen().as_str(), "play" | "matches") {
            return;
        }
        self.spawn_named("page", move |ui| {
            let screen = ui.screen();
            let app = ui.app();
            let Some(view) = ui.with_page(&screen, |p| p.view(&app)) else { return };
            *lock(&ui.pages.refreshed) = Instant::now();
            // Only if it's still the screen shown.
            if ui.screen() == screen {
                ui.push_page(view);
            }
        });
    }

    fn push_page(self: &Arc<Self>, view: PageView) {
        let _ = self.window().upgrade_in_event_loop(move |w| {
            w.set_page_kicker(s(&view.kicker));
            w.set_page_title(s(&view.title));
            w.set_page_summary(s(&view.summary));
            w.set_page_sections(ModelRc::new(VecModel::from(view.sections.iter().map(|(_, l)| s(l)).collect::<Vec<_>>())));
            w.set_page_section_ids(ModelRc::new(VecModel::from(view.sections.iter().map(|(i, _)| s(i)).collect::<Vec<_>>())));
            w.set_page_section(s(&view.section));
            set_rows(&w.get_page_rows(), &view.rows, |m| w.set_page_rows(m));
        });
    }

    /// Something done on the page shown: `f`, then the page again.
    pub fn on_page(self: &Arc<Self>, f: impl FnOnce(&AppHandle, &mut dyn Screen) -> bool + Send + 'static) {
        let screen = self.screen();
        self.act(move |ui| {
            let app = ui.app();
            let again = ui.with_page(&screen, |p| f(&app, p)).unwrap_or(false);
            if again {
                ui.load_page();
                ui.load_info();
            }
        });
    }

    // The dialog.

    /// Shows `dialog`, or queues it behind the one up.
    pub fn open_dialog(self: &Arc<Self>, dialog: Dialog) {
        {
            let mut up = lock(&self.pages.dialog);
            if up.is_some() {
                lock(&self.pages.queue).push(dialog);
                return;
            }
            *up = Some(dialog);
        }
        self.load_dialog();
    }

    pub fn load_dialog(self: &Arc<Self>) {
        self.spawn_named("dialog", |ui| {
            let app = ui.app();
            let view = {
                let mut up = lock(&ui.pages.dialog);
                let Some(d) = up.as_mut() else {
                    let _ = ui.window().upgrade_in_event_loop(|w| w.set_dialog_shown(false));
                    return;
                };
                let view = d.view(&app);
                if view.closed || d.closed() {
                    *up = lock(&ui.pages.queue).pop();
                    None
                } else {
                    Some(view)
                }
            };
            match view {
                None => {
                    let _ = ui.window().upgrade_in_event_loop(|w| w.set_dialog_shown(false));
                    if lock(&ui.pages.dialog).is_some() {
                        ui.load_dialog();
                    }
                    ui.load_info();
                }
                Some(v) => {
                    let _ = ui.window().upgrade_in_event_loop(move |w| {
                        w.set_dialog_kicker(s(&v.page.kicker));
                        w.set_dialog_title(s(&v.page.title));
                        w.set_dialog_width(v.width);
                        set_rows(&w.get_dialog_rows(), &v.page.rows, |m| w.set_dialog_rows(m));
                        w.set_dialog_shown(true);
                    });
                }
            }
        });
    }

    pub fn on_dialog(self: &Arc<Self>, f: impl FnOnce(&AppHandle, &mut Dialog) + Send + 'static) {
        self.act(move |ui| {
            let app = ui.app();
            if let Some(d) = lock(&ui.pages.dialog).as_mut() {
                f(&app, d);
            }
            ui.load_dialog();
        });
    }

    /// The questions before a first game, What's new and a report asked for: whatever's due.
    pub fn check_dialogs(self: &Arc<Self>, at_start: bool) {
        self.spawn_named("dialogs", move |ui| {
            let app = ui.app();
            if crate::pages::dialogs::questions_pending(&app) && !lock(&ui.pages.setup).fresh {
                let already = matches!(lock(&ui.pages.dialog).as_ref(), Some(Dialog::Questions(_)));
                if !already {
                    ui.open_dialog(Dialog::Questions(Default::default()));
                }
            }
            if at_start {
                if let Some(view) = crate::whats_new::whats_new(app.clone(), app.state::<crate::whats_new::WhatsNew>()) {
                    ui.open_dialog(Dialog::WhatsNew {
                        view: crate::pages::json(&view),
                        on_start: true,
                        closed: false,
                    });
                }
            }
        });
    }

    /// An event from a command module (as the Tauri page listened for it).
    pub fn event(self: &Arc<Self>, name: &str) {
        let screen = self.screen();
        match name {
            "chat" => {
                if screen == "chat" {
                    self.load_page();
                }
                self.load_glance();
            }
            "support" | "support-sent" if screen == "support" => self.load_page(),
            "settings" if screen == "settings" => self.load_page(),
            "feedback" => {
                let app = self.app();
                if crate::feedback::feedback(app.state::<crate::feedback::Feedback>()).is_some() {
                    let up = matches!(lock(&self.pages.dialog).as_ref(), Some(Dialog::Feedback(_)));
                    if !up {
                        self.open_dialog(Dialog::Feedback(Default::default()));
                    }
                }
            }
            "update" => self.load_banner(),
            "setup-done" => self.check_dialogs(false),
            "checks" => self.reload_play(),
            _ => {}
        }
    }

    /// Chat at a glance: unseen mentions and messages (the side menu's dot).
    pub fn load_glance(self: &Arc<Self>) {
        let app = self.app();
        let glance = crate::pages::json(&crate::chat::chat_glance(app.state::<crate::chat::Chat>()));
        let unread = crate::pages::num(&glance, "unread") > 0.0;
        let _ = self.window().upgrade_in_event_loop(move |w| w.set_chat_unread(unread));
    }

    /// The update banner, when a newer release waits.
    pub fn load_banner(self: &Arc<Self>) {
        let app = self.app();
        let banner = crate::update::update_banner(app.clone(), app.state::<crate::update::Update>()).map(|b| crate::pages::json(&b));
        let _ = self.window().upgrade_in_event_loop(move |w| match banner {
            Some(b) if crate::pages::bo(&b, "updating") => {
                w.set_banner(s(format!("Updating to {}: the launcher restarts when it's ready.", crate::pages::st(&b, "version"))));
                w.set_banner_button(s("Updating…"));
            }
            Some(b) => {
                w.set_banner(s(format!("Version {} is out.", crate::pages::st(&b, "version"))));
                w.set_banner_button(s("Update now"));
            }
            None => w.set_banner(s("")),
        });
    }

    /// What a page asks of the window.
    pub fn window_call(self: &Arc<Self>, call: WindowCall) -> bool {
        match call {
            WindowCall::IsFocused => return self.pages.focused.load(Ordering::Relaxed),
            WindowCall::RequestAttention => {
                let _ = self.window().upgrade_in_event_loop(|w| {
                    use slint::winit_030::WinitWindowAccessor as _;
                    w.window()
                        .with_winit_window(|win| win.request_user_attention(Some(slint::winit_030::winit::window::UserAttentionType::Informational)));
                });
            }
            WindowCall::Copy(text) => {
                let _ = self.window().upgrade_in_event_loop(move |w| w.invoke_copy_text(s(text)));
            }
            WindowCall::Size(size) => self.apply_size(size),
            WindowCall::FullScreen => {
                let _ = self.window().upgrade_in_event_loop(|w| {
                    let to = !w.window().is_fullscreen();
                    w.window().set_fullscreen(to);
                });
            }
            WindowCall::Theme(theme) => {
                let choice = match theme {
                    launcher::prefs::Theme::Dark => crate::ThemeChoice::Dark,
                    launcher::prefs::Theme::Light => crate::ThemeChoice::Light,
                    launcher::prefs::Theme::System => crate::ThemeChoice::System,
                };
                let _ = self.window().upgrade_in_event_loop(move |w| w.global::<crate::Theme>().set_choice(choice));
            }
            WindowCall::Go(screen) => self.go(&screen),
            WindowCall::Account { server, suggested, taken, own } => {
                self.open_dialog(Dialog::Account(crate::pages::dialogs::Account::new(server, suggested, taken, own)));
            }
            WindowCall::Feedback => {
                let app = self.app();
                crate::feedback::ask_feedback(app.clone(), app.state::<crate::feedback::Feedback>());
            }
            WindowCall::WhatsNew => {
                if let Some(view) = crate::whats_new::whats_new_again() {
                    self.open_dialog(Dialog::WhatsNew {
                        view: crate::pages::json(&view),
                        on_start: false,
                        closed: false,
                    });
                }
            }
            WindowCall::Quit => {
                let _ = self.window().upgrade_in_event_loop(|w| {
                    let _ = w.hide();
                });
            }
        }
        true
    }

    /// The player's size for the launcher, on top of the system's own scaling.
    pub fn apply_size(self: &Arc<Self>, size: f32) {
        let base = self.base_scale();
        let _ = self.window().upgrade_in_event_loop(move |w| {
            // The window keeps its size on screen; what's in it gets bigger or smaller: the
            // same pixels, fewer (or more) logical ones.
            let keep = w.window().size();
            let scale = base * size;
            w.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale });
            w.window().dispatch_event(slint::platform::WindowEvent::Resized { size: keep.to_logical(scale) });
        });
    }
}

/// The window's callbacks for the pages, the dialog and the banner.
pub fn wire(w: &AppWindow, ui: &Arc<Ui>) {
    w.on_page_section_chosen({
        let ui = ui.clone();
        move |id| {
            let id = id.to_string();
            ui.on_page(move |_, p| {
                p.section(&id);
                true
            })
        }
    });
    w.on_page_toggled({
        let ui = ui.clone();
        move |id, on| {
            let id = id.to_string();
            ui.on_page(move |app, p| {
                p.toggled(app, &id, on);
                true
            })
        }
    });
    w.on_page_chosen({
        let ui = ui.clone();
        move |id, i| {
            let id = id.to_string();
            ui.on_page(move |app, p| {
                p.chosen(app, &id, i.max(0) as usize);
                true
            })
        }
    });
    w.on_page_slid({
        let ui = ui.clone();
        move |id, v| {
            let id = id.to_string();
            ui.on_page(move |app, p| {
                p.slid(app, &id, v);
                true
            })
        }
    });
    w.on_page_edited({
        let ui = ui.clone();
        move |id, text| {
            let (id, text) = (id.to_string(), text.to_string());
            ui.on_page(move |_, p| {
                p.edited(&id, &text);
                p.live_edits()
            })
        }
    });
    w.on_page_accepted({
        let ui = ui.clone();
        move |id, text| {
            let (id, text) = (id.to_string(), text.to_string());
            ui.on_page(move |app, p| {
                p.accepted(app, &id, &text);
                true
            })
        }
    });
    w.on_page_pressed({
        let ui = ui.clone();
        move |id| {
            let id = id.to_string();
            ui.on_page(move |app, p| {
                p.pressed(app, &id);
                true
            })
        }
    });

    w.on_dialog_toggled({
        let ui = ui.clone();
        move |id, on| {
            let id = id.to_string();
            ui.on_dialog(move |app, d| d.toggled(app, &id, on))
        }
    });
    w.on_dialog_chosen({
        let ui = ui.clone();
        move |id, i| {
            let id = id.to_string();
            ui.on_dialog(move |app, d| d.chosen(app, &id, i.max(0) as usize))
        }
    });
    w.on_dialog_slid(|_, _| {});
    w.on_dialog_edited({
        let ui = ui.clone();
        move |id, text| {
            let (id, text) = (id.to_string(), text.to_string());
            ui.act(move |ui| {
                let mut up = lock(&ui.pages.dialog);
                let live = up.as_ref().is_some_and(Dialog::live_edits);
                if let Some(d) = up.as_mut() {
                    d.edited(&id, &text);
                }
                drop(up);
                if live {
                    ui.load_dialog();
                }
            })
        }
    });
    w.on_dialog_accepted({
        let ui = ui.clone();
        move |id, text| {
            let (id, text) = (id.to_string(), text.to_string());
            ui.on_dialog(move |app, d| {
                d.edited(&id, &text);
                d.pressed(app, &id);
            })
        }
    });
    w.on_dialog_pressed({
        let ui = ui.clone();
        move |id| {
            let id = id.to_string();
            ui.on_dialog(move |app, d| d.pressed(app, &id))
        }
    });

    w.on_shortcut({
        let ui = ui.clone();
        move |what| {
            let what = what.to_string();
            ui.act(move |ui| {
                let app = ui.app();
                let mut settings = lock(&ui.pages.settings);
                settings.pressed(&app, &what);
                drop(settings);
                if ui.screen() == "settings" {
                    ui.load_page();
                }
            })
        }
    });
    w.on_banner_now({
        let ui = ui.clone();
        move || {
            let ui = ui.clone();
            std::thread::spawn(move || {
                let app = ui.app();
                crate::update::update_now(app.clone(), app.state::<crate::update::Update>());
                ui.load_banner();
            });
        }
    });
    w.on_banner_later({
        let ui = ui.clone();
        move || {
            let app = ui.app();
            crate::update::update_later(app.state::<crate::update::Update>());
            ui.load_banner();
        }
    });
}
