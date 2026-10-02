//! The in-game overlay: "5th Echelon Enhanced" in its dark Echelon theme.
//!
//! - A toast when the game starts (how to open the overlay), one for each
//!   invite, and a banner when the server can't be reached.
//! - F5 opens a panel (F5 or Esc closes it) with Players, Invites, Match (the
//!   lobby's player counts) and Server.
//!
//! Sizes are designed for 1080p and scale with the screen height. Fonts are
//! rasterised at twice that size so they stay sharp up to 4K.

use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use hudhook::ImguiRenderLoop;
use imgui::Condition;
use imgui::FontId;
use imgui::StyleColor;
use imgui::StyleVar;
use imgui::Ui;
use server_api::misc::InviteEvent;
use server_api::users::User;
use tracing::info;
use windows::core::PCSTR;
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_F5;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcA;

use crate::community;
use crate::uplay_r1_loader::Event;
use crate::uplay_r1_loader::EVENTS;

/// Our name and release, from release.toml.
const PRODUCT: &str = env!("FE_PRODUCT");
const RELEASE: &str = env!("FE_RELEASE");

static NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(30);
static INITIAL_POPUP_DURATION: Duration = Duration::from_secs(10);
static NOTICE_DURATION: Duration = Duration::from_secs(5);
/// How often the player list refreshes while the panel is open (it's every
/// 15 s otherwise), so friends starting their game show up quickly.
static PANEL_REFRESH: Duration = Duration::from_secs(5);

/// The screen height the sizes below are designed for.
const DESIGN_HEIGHT: f32 = 1080.0;
/// Fonts are rasterised at this multiple of their 1080p size.
const FONT_OVERSAMPLE: f32 = 2.0;
const BODY_PX: f32 = 17.0;
const HEADING_PX: f32 = 22.0;

// Echelon, dark: slate greys with night-vision green as the accent.
const BG: [f32; 4] = rgba(0x0f1518, 0.94);
const SURFACE: [f32; 4] = rgba(0x1c252a, 1.0);
const SURFACE_HOVER: [f32; 4] = rgba(0x273238, 1.0);
const LINE: [f32; 4] = [1.0, 1.0, 1.0, 0.09];
const ROW: [f32; 4] = [1.0, 1.0, 1.0, 0.03];
const FG: [f32; 4] = rgba(0xe6edf0, 1.0);
const MUTED: [f32; 4] = rgba(0x93a4ad, 1.0);
const OFFLINE: [f32; 4] = rgba(0x93a4ad, 0.5);
const ACCENT: [f32; 4] = rgba(0x8fd14f, 1.0);
const ACCENT_HOVER: [f32; 4] = rgba(0xa7e070, 1.0);
const ACCENT_SOFT: [f32; 4] = rgba(0x8fd14f, 0.16);
const ON_ACCENT: [f32; 4] = rgba(0x0b1405, 1.0);
const OK: [f32; 4] = rgba(0x3ecf9e, 1.0);
const BAD: [f32; 4] = rgba(0xff6b6b, 1.0);
const DIM: [f32; 4] = [0.0, 0.02, 0.03, 0.55];

#[allow(clippy::cast_precision_loss)]
const fn rgba(hex: u32, alpha: f32) -> [f32; 4] {
    [((hex >> 16) & 0xff) as f32 / 255.0, ((hex >> 8) & 0xff) as f32 / 255.0, (hex & 0xff) as f32 / 255.0, alpha]
}

fn theme_colors(style: &mut imgui::Style) {
    let c = &mut style.colors;
    c[StyleColor::Text as usize] = FG;
    c[StyleColor::TextDisabled as usize] = MUTED;
    c[StyleColor::WindowBg as usize] = BG;
    c[StyleColor::ChildBg as usize] = [0.0, 0.0, 0.0, 0.0];
    c[StyleColor::PopupBg as usize] = BG;
    c[StyleColor::Border as usize] = LINE;
    c[StyleColor::BorderShadow as usize] = [0.0, 0.0, 0.0, 0.0];
    c[StyleColor::FrameBg as usize] = SURFACE;
    c[StyleColor::FrameBgHovered as usize] = SURFACE_HOVER;
    c[StyleColor::FrameBgActive as usize] = SURFACE_HOVER;
    c[StyleColor::TitleBg as usize] = BG;
    c[StyleColor::TitleBgActive as usize] = BG;
    c[StyleColor::TitleBgCollapsed as usize] = BG;
    c[StyleColor::ScrollbarBg as usize] = [0.0, 0.0, 0.0, 0.0];
    c[StyleColor::ScrollbarGrab as usize] = SURFACE;
    c[StyleColor::ScrollbarGrabHovered as usize] = SURFACE_HOVER;
    c[StyleColor::ScrollbarGrabActive as usize] = SURFACE_HOVER;
    c[StyleColor::CheckMark as usize] = ACCENT;
    c[StyleColor::SliderGrab as usize] = ACCENT;
    c[StyleColor::SliderGrabActive as usize] = ACCENT_HOVER;
    c[StyleColor::Button as usize] = SURFACE;
    c[StyleColor::ButtonHovered as usize] = SURFACE_HOVER;
    c[StyleColor::ButtonActive as usize] = SURFACE_HOVER;
    c[StyleColor::Header as usize] = ACCENT_SOFT;
    c[StyleColor::HeaderHovered as usize] = ACCENT_SOFT;
    c[StyleColor::HeaderActive as usize] = ACCENT_SOFT;
    c[StyleColor::Separator as usize] = LINE;
    c[StyleColor::SeparatorHovered as usize] = LINE;
    c[StyleColor::SeparatorActive as usize] = LINE;
    c[StyleColor::ResizeGrip as usize] = [0.0, 0.0, 0.0, 0.0];
    c[StyleColor::ResizeGripHovered as usize] = [0.0, 0.0, 0.0, 0.0];
    c[StyleColor::ResizeGripActive as usize] = [0.0, 0.0, 0.0, 0.0];
    c[StyleColor::TextSelectedBg as usize] = ACCENT_SOFT;
    c[StyleColor::NavHighlight as usize] = ACCENT;
}

/// Sizes at 1080p, multiplied by `s` for the current screen.
fn theme_sizes(style: &mut imgui::Style, s: f32) {
    style.window_rounding = 10.0 * s;
    style.child_rounding = 8.0 * s;
    style.frame_rounding = 7.0 * s;
    style.popup_rounding = 8.0 * s;
    style.grab_rounding = 7.0 * s;
    style.scrollbar_rounding = 7.0 * s;
    style.window_border_size = 1.0;
    style.child_border_size = 0.0;
    style.frame_border_size = 0.0;
    style.window_padding = [18.0 * s, 16.0 * s];
    style.frame_padding = [14.0 * s, 9.0 * s];
    style.item_spacing = [10.0 * s, 10.0 * s];
    style.item_inner_spacing = [8.0 * s, 6.0 * s];
    style.scrollbar_size = 10.0 * s;
}

struct Fonts {
    strong: FontId,
    heading: FontId,
}

// SAFETY: a FontId is a pointer into the font atlas, which the imgui context
// owns for as long as the overlay lives. The ids are only used on the render
// thread (in `render`), never dereferenced elsewhere; hudhook merely requires
// the render loop to be Send + Sync.
unsafe impl Send for Fonts {}
unsafe impl Sync for Fonts {}

fn add_fonts(ctx: &mut imgui::Context) -> Fonts {
    let mut add = |data: &'static [u8], px: f32, name: &str| {
        ctx.fonts().add_font(&[imgui::FontSource::TtfData {
            data,
            size_pixels: px * FONT_OVERSAMPLE,
            config: Some(imgui::FontConfig {
                name: Some(String::from(name)),
                ..imgui::FontConfig::default()
            }),
        }])
    };
    // The first font added is imgui's default: body text.
    add(include_bytes!("../fonts/IBMPlexSans-Regular.ttf"), BODY_PX, "IBM Plex Sans");
    let strong = add(include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf"), BODY_PX, "IBM Plex Sans SemiBold");
    let heading = add(include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf"), HEADING_PX, "IBM Plex Sans SemiBold (heading)");
    Fonts { strong, heading }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum Engine {
    DX9,
    DX11,
}

impl Engine {
    pub fn detect() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let fname = exe.file_name()?.to_str()?.to_lowercase();
        match fname.as_str() {
            "blacklist_game.exe" => Some(Self::DX9),
            "blacklist_dx11_game.exe" => Some(Self::DX11),
            _ => None,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum UiState {
    Show,
    #[default]
    Hide,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Tab {
    #[default]
    Friends,
    Players,
    Invites,
    Match,
    Server,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Friends, Tab::Players, Tab::Invites, Tab::Match, Tab::Server];

    fn label(self) -> &'static str {
        match self {
            Tab::Friends => "Friends",
            Tab::Players => "Find players",
            Tab::Invites => "Invites",
            Tab::Match => "Match",
            Tab::Server => "Server",
        }
    }
}

/// A stable number for a server-given id, for ImGui labels.
fn ui_id(id: &str) -> u64 {
    use std::hash::Hash as _;
    use std::hash::Hasher as _;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

/// A row button and what it does to the player on that row.
#[derive(Clone, Copy)]
enum RowAction {
    Invite,
    Change(community::FriendChange, &'static str),
}

struct Invite {
    event: InviteEvent,
    clicked: bool,
    received: Instant,
}

/// A short result message of our own (signing in again).
struct LocalNotice {
    text: String,
    error: bool,
    at: Instant,
}

struct MyRenderLoop {
    tx: mpsc::Sender<Event>,
    /// For the developer window (OverlayDebug).
    username: String,
    ui_state: UiState,
    tab: Tab,
    f5_down: bool,
    esc_down: bool,
    invite_notification: Option<(Instant, String)>,
    new_invites: crossbeam_channel::Receiver<Result<Option<InviteEvent>, crate::api::Error>>,
    active_invites: Vec<Invite>,
    connection_error: Option<crate::api::Error>,
    initial_popup: Instant,
    fonts: Option<Fonts>,
    /// Screen scale for this frame (1.0 at 1080p).
    s: f32,
    data: community::Snapshot,
    local_notice: Option<LocalNotice>,
    relogin: Option<std::thread::JoinHandle<bool>>,
    /// When the panel last asked for fresh data.
    last_refresh: Instant,
    /// What's typed in Find players.
    search_text: String,
    /// Whether Find players has searched since the panel opened.
    searched: bool,
}

/// Builds the Uplay event that hands an accepted invitation to the game.
///
/// The event kind decides which join route the game takes from there; see
/// [`hooks_config::InviteAcceptEvent`].
fn invite_accept_event(user_id: String) -> Event {
    match hooks_config::get().map(|cfg| cfg.invite_accept_event).unwrap_or_default() {
        hooks_config::InviteAcceptEvent::Friends => Event::FriendsGameInviteAccepted(user_id),
        hooks_config::InviteAcceptEvent::Party => Event::PartyGameInviteAccepted(user_id),
    }
}

/// Where the player fixes their setup: the tool that manages this install
/// (the override file's `[Managed]`), or the launcher.
fn setup_tool() -> String {
    hooks_config::get()
        .and_then(|cfg| cfg.managed.as_ref())
        .map_or_else(|| String::from("the 5th Echelon launcher"), |m| m.by.clone())
}

/// What to do when the server can't be reached: the managing tool's own
/// advice if it gave some.
fn server_fix_hint() -> String {
    hooks_config::get()
        .and_then(|cfg| cfg.managed.as_ref())
        .and_then(|m| m.help_text.clone())
        .unwrap_or_else(|| format!("If this lasts, open {} and check that the server shows as online.", setup_tool()))
}

/// What a connection error means for the player, and what to do about it.
fn describe_error(err: &crate::api::Error) -> (&'static str, String) {
    let fix = server_fix_hint();
    let tool = setup_tool();
    match err {
        crate::api::Error::MissingUrl => ("5th Echelon isn't set up", format!("No server is set for the game. Open {tool} and set up a server.")),
        crate::api::Error::LoginFailure | crate::api::Error::InvalidToken(_) => (
            "Couldn't sign in to the server",
            format!("Your account or password was refused. Open {tool} and sign in again."),
        ),
        // Calls sign in again on their own; this is what's left when that failed too.
        crate::api::Error::GRPCStatus(e) if e.code() == tonic::Code::Unauthenticated => {
            ("Signed out by the server", format!("Signing in again didn't work. Open {tool} and sign in again."))
        }
        _ => ("Lost the 5th Echelon server", format!("Reconnecting now. Invites are paused until it's back. {fix}")),
    }
}

impl MyRenderLoop {
    fn s(&self, v: f32) -> f32 {
        v * self.s
    }

    fn with_font<R>(&self, ui: &Ui, pick: impl FnOnce(&Fonts) -> FontId, f: impl FnOnce() -> R) -> R {
        match &self.fonts {
            Some(fonts) => {
                let _t = ui.push_font(pick(fonts));
                f()
            }
            None => f(),
        }
    }

    fn join_session(&self, sender: &User) {
        let event = invite_accept_event(sender.id.clone());
        info!("Invitation accepted, event: {event:?}");
        let _ = self.tx.send(event);
    }

    fn toggle(&mut self) {
        self.ui_state = match self.ui_state {
            UiState::Show => UiState::Hide,
            UiState::Hide => {
                community::refresh();
                self.last_refresh = Instant::now();
                // Open where there's something to act on.
                if !self.active_invites.is_empty() {
                    self.tab = Tab::Invites;
                } else if !self.data.requests_in.is_empty() {
                    self.tab = Tab::Friends;
                }
                self.searched = false;
                UiState::Show
            }
        };
    }

    fn poll_keys(&mut self) {
        #[allow(clippy::cast_possible_wrap)]
        let down = |vk: u16| unsafe { GetAsyncKeyState(vk.into()) & 0x8000u16 as i16 != 0 };
        let f5 = down(VK_F5.0);
        if f5 && !self.f5_down {
            self.toggle();
        }
        self.f5_down = f5;
        let esc = down(VK_ESCAPE.0);
        if esc && !self.esc_down && self.ui_state == UiState::Show {
            self.ui_state = UiState::Hide;
        }
        self.esc_down = esc;
    }

    fn poll_invites(&mut self) {
        if let Ok(evt) = self.new_invites.try_recv() {
            match evt {
                Err(e) => self.connection_error = Some(e),
                Ok(evt) => {
                    self.connection_error = None;
                    if let Some(mut evt) = evt {
                        // The name is shown in the toast and the Invites tab: as any server text, cut and cleaned.
                        if let Some(sender) = evt.sender.as_mut() {
                            sender.username = hooks_config::text::clip(&sender.username, 32);
                        }
                        if let Some(ref sender) = evt.sender {
                            self.invite_notification.replace((Instant::now(), sender.username.clone()));
                        }
                        // The server may ask to join without a click only if the player allowed it.
                        let cfg = hooks_config::get();
                        let force_join = evt.force_join && cfg.is_some_and(|c| c.allow_force_join);
                        if evt.sender.is_some() && (force_join || cfg.is_some_and(|c| c.auto_join_invite)) {
                            self.join_session(&evt.sender.unwrap());
                        } else {
                            self.active_invites.push(Invite {
                                event: evt,
                                clicked: force_join,
                                received: Instant::now(),
                            });
                        }
                    }
                }
            }
        }
        // Answered invites go; unanswered ones expire with their notification.
        self.active_invites.retain(|i| !i.clicked && i.received.elapsed() < NOTIFICATION_TIMEOUT);
    }

    fn poll_relogin(&mut self) {
        if self.relogin.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            let ok = self.relogin.take().and_then(|h| h.join().ok()).unwrap_or(false);
            self.local_notice = Some(LocalNotice {
                text: if ok {
                    String::from("Signed in again")
                } else {
                    format!("Couldn't sign in. {}", server_fix_hint())
                },
                error: !ok,
                at: Instant::now(),
            });
            community::refresh();
        }
    }

    // ---------- Toasts and banners ----------

    /// A small window in the top-right corner. `progress` (1 down to 0) draws
    /// the draining bar along its bottom edge.
    fn toast(&self, ui: &Ui, id: &str, progress: Option<f32>, body: impl FnOnce()) {
        let win = ui.io().display_size;
        let margin = self.s(20.0);
        let _p = ui.push_style_var(StyleVar::WindowPadding([self.s(18.0), self.s(14.0)]));
        ui.window(id)
            .no_decoration()
            .no_inputs()
            .no_nav()
            .movable(false)
            .focus_on_appearing(false)
            .always_auto_resize(true)
            .size([self.s(400.0), 0.0], Condition::Always)
            .position([win[0] - margin, margin], Condition::Always)
            .position_pivot([1.0, 0.0])
            .build(|| {
                body();
                if let Some(p) = progress {
                    let pos = ui.window_pos();
                    let size = ui.window_size();
                    let h = self.s(3.0);
                    ui.get_window_draw_list()
                        .add_rect([pos[0], pos[1] + size[1] - h], [pos[0] + size[0] * p.clamp(0.0, 1.0), pos[1] + size[1]], ACCENT)
                        .filled(true)
                        .build();
                }
            });
    }

    fn heading(&self, ui: &Ui, text: &str) {
        self.with_font(ui, |f| f.heading, || ui.text(text));
    }

    /// A key cap, e.g. F5, inline with the text.
    fn key(&self, ui: &Ui, key: &str) {
        let pad = [self.s(6.0), self.s(2.0)];
        let size = ui.calc_text_size(key);
        let pos = ui.cursor_screen_pos();
        let dl = ui.get_window_draw_list();
        dl.add_rect(pos, [pos[0] + size[0] + pad[0] * 2.0, pos[1] + size[1] + pad[1] * 2.0], ACCENT)
            .filled(true)
            .rounding(self.s(4.0))
            .build();
        dl.add_text([pos[0] + pad[0], pos[1] + pad[1]], ON_ACCENT, key);
        ui.dummy([size[0] + pad[0] * 2.0, size[1] + pad[1] * 2.0]);
    }

    fn show_initial_info(&self, ui: &Ui) {
        let Some(left) = self.initial_popup.checked_duration_since(Instant::now()) else {
            return;
        };
        if self.ui_state == UiState::Show || self.invite_notification.is_some() {
            return;
        }
        let progress = left.as_secs_f32() / INITIAL_POPUP_DURATION.as_secs_f32();
        self.toast(ui, "##fe-start", Some(progress), || {
            self.heading(ui, PRODUCT);
            let who = crate::api::username().map_or_else(|| String::from("Connecting"), |u| format!("Signed in as {u}"));
            let sub = if self.data.loaded {
                match self.data.online_count() {
                    0 => format!("{who}. No friends online yet."),
                    1 => format!("{who}. 1 friend online."),
                    n => format!("{who}. {n} friends online."),
                }
            } else {
                who
            };
            ui.text_colored(MUTED, sub);
            ui.text_colored(MUTED, "Press");
            ui.same_line();
            self.key(ui, "F5");
            ui.same_line();
            ui.text_colored(MUTED, "for friends, invites and match settings");
        });
    }

    fn show_invite_toast(&mut self, ui: &Ui) {
        let Some((received, user)) = self.invite_notification.clone() else {
            return;
        };
        let elapsed = received.elapsed();
        if elapsed > NOTIFICATION_TIMEOUT {
            self.invite_notification = None;
            return;
        }
        if self.ui_state == UiState::Show {
            return;
        }
        let left = NOTIFICATION_TIMEOUT - elapsed;
        let activity = self.data.activity_of(&user).map(String::from);
        let lookalike = self.data.lookalike_of(&user);
        let pending = self.active_invites.iter().any(|i| i.event.sender.as_ref().is_some_and(|s| s.username == user));
        self.toast(ui, "##fe-invite", Some(left.as_secs_f32() / NOTIFICATION_TIMEOUT.as_secs_f32()), || {
            self.heading(ui, &format!("{user} invited you"));
            if let Some(server) = &lookalike {
                ui.text_colored(BAD, format!("Not your friend {user} from {server}"));
            }
            if let Some(activity) = &activity {
                ui.text_colored(MUTED, activity);
            }
            if pending {
                self.key(ui, "F5");
                ui.same_line();
                ui.text_colored(MUTED, format!("accept or decline  ·  {} s", left.as_secs()));
            } else {
                ui.text_colored(MUTED, "Joining their match...");
            }
        });
    }

    fn show_error_banner(&self, ui: &Ui) {
        let Some(err) = &self.connection_error else {
            return;
        };
        let (title, detail) = describe_error(err);
        let win = ui.io().display_size;
        let _p = ui.push_style_var(StyleVar::WindowPadding([self.s(22.0), self.s(14.0)]));
        ui.window("##fe-error")
            .no_decoration()
            .no_inputs()
            .no_nav()
            .movable(false)
            .focus_on_appearing(false)
            .always_auto_resize(true)
            .size([self.s(620.0), 0.0], Condition::Always)
            .position([win[0] / 2.0, self.s(20.0)], Condition::Always)
            .position_pivot([0.5, 0.0])
            .build(|| {
                let pos = ui.window_pos();
                let size = ui.window_size();
                ui.get_window_draw_list().add_rect(pos, [pos[0] + self.s(4.0), pos[1] + size[1]], BAD).filled(true).build();
                self.with_font(ui, |f| f.heading, || ui.text_colored(BAD, title));
                let _c = ui.push_style_color(StyleColor::Text, MUTED);
                ui.text_wrapped(detail);
            });
    }

    /// Short results (invite sent, signed in again) at the bottom centre.
    fn show_notice(&self, ui: &Ui) {
        let shared = self.data.notice.as_ref().map(|n| (n.text.as_str(), n.error, n.at));
        let local = self.local_notice.as_ref().map(|n| (n.text.as_str(), n.error, n.at));
        let latest = [shared, local]
            .into_iter()
            .flatten()
            .filter(|(_, _, at)| at.elapsed() < NOTICE_DURATION)
            .max_by_key(|(_, _, at)| *at);
        let Some((text, error, _)) = latest else {
            return;
        };
        let win = ui.io().display_size;
        ui.window("##fe-notice")
            .no_decoration()
            .no_inputs()
            .no_nav()
            .movable(false)
            .focus_on_appearing(false)
            .always_auto_resize(true)
            .position([win[0] / 2.0, win[1] - self.s(48.0)], Condition::Always)
            .position_pivot([0.5, 1.0])
            .build(|| {
                ui.text_colored(if error { BAD } else { FG }, text);
            });
    }

    // ---------- The panel ----------

    fn show_panel(&mut self, ui: &Ui) {
        let win = ui.io().display_size;
        ui.get_background_draw_list().add_rect([0.0, 0.0], win, DIM).filled(true).build();

        let size = [self.s(900.0).min(win[0] - self.s(40.0)), self.s(620.0).min(win[1] - self.s(40.0))];
        let _p = ui.push_style_var(StyleVar::WindowPadding([0.0, 0.0]));
        let _r = ui.push_style_var(StyleVar::WindowRounding(self.s(12.0)));
        ui.window("5th Echelon Enhanced##fe-panel")
            .no_decoration()
            .movable(false)
            .resizable(false)
            .size(size, Condition::Always)
            .position([win[0] / 2.0, win[1] / 2.0], Condition::Always)
            .position_pivot([0.5, 0.5])
            .build(|| {
                let header_h = self.s(64.0);
                let footer_h = self.s(46.0);
                self.panel_header(ui, size[0], header_h);
                let body_h = size[1] - header_h - footer_h;
                ui.set_cursor_pos([0.0, header_h]);
                ui.child_window("##fe-nav").size([self.s(200.0), body_h]).build(|| self.panel_nav(ui));
                ui.same_line_with_spacing(0.0, 0.0);
                {
                    let _cp = ui.push_style_var(StyleVar::WindowPadding([self.s(26.0), self.s(20.0)]));
                    ui.child_window("##fe-content")
                        .size([0.0, body_h])
                        .always_use_window_padding(true)
                        .build(|| match self.tab {
                            Tab::Friends => self.pane_friends(ui),
                            Tab::Players => self.pane_players(ui),
                            Tab::Invites => self.pane_invites(ui),
                            Tab::Match => self.pane_match(ui),
                            Tab::Server => self.pane_server(ui),
                        });
                }
                self.panel_footer(ui, size, footer_h);
            });
    }

    fn panel_header(&self, ui: &Ui, width: f32, h: f32) {
        let pos = ui.window_pos();
        ui.get_window_draw_list().add_line([pos[0], pos[1] + h], [pos[0] + width, pos[1] + h], LINE).build();
        let pad = self.s(24.0);

        // Name and release on the left.
        let title_h = self.with_font(ui, |f| f.heading, || ui.calc_text_size(PRODUCT))[1];
        ui.set_cursor_pos([pad, (h - title_h) / 2.0]);
        self.heading(ui, PRODUCT);
        ui.same_line();
        let body_h = ui.text_line_height();
        ui.set_cursor_pos([ui.cursor_pos()[0], (h - body_h) / 2.0]);
        ui.text_colored(MUTED, RELEASE);

        // Connection on the right.
        let (dot, status) = if self.connection_error.is_some() {
            (BAD, String::from("Not connected"))
        } else {
            let ms = self.data.response_time.map(|d| format!("{} ms", d.as_millis()));
            let parts: Vec<String> = [Some(String::from("Connected")), ms, crate::api::username()].into_iter().flatten().collect();
            (OK, parts.join("  ·  "))
        };
        let text_w = ui.calc_text_size(&status)[0];
        let x = width - pad - text_w;
        let r = self.s(4.5);
        ui.get_window_draw_list().add_circle([pos[0] + x - r * 3.0, pos[1] + h / 2.0], r, dot).filled(true).build();
        ui.set_cursor_pos([x, (h - body_h) / 2.0]);
        ui.text_colored(MUTED, status);
    }

    fn panel_nav(&mut self, ui: &Ui) {
        let pos = ui.window_pos();
        let size = ui.window_size();
        ui.get_window_draw_list()
            .add_line([pos[0] + size[0] - 1.0, pos[1]], [pos[0] + size[0] - 1.0, pos[1] + size[1]], LINE)
            .build();
        let pad = self.s(12.0);
        let item_h = self.s(44.0);
        let pending = self.active_invites.len();
        #[allow(clippy::cast_precision_loss)]
        for (i, tab) in Tab::ALL.into_iter().enumerate() {
            let top = pad + i as f32 * (item_h + self.s(4.0));
            ui.set_cursor_pos([pad, top]);
            let p = ui.cursor_screen_pos();
            let w = size[0] - pad * 2.0;
            if ui.invisible_button(format!("##fe-tab-{i}"), [w, item_h]) {
                self.tab = tab;
            }
            let hovered = ui.is_item_hovered();
            let selected = self.tab == tab;
            let dl = ui.get_window_draw_list();
            if selected || hovered {
                dl.add_rect(p, [p[0] + w, p[1] + item_h], if selected { ACCENT_SOFT } else { ROW })
                    .filled(true)
                    .rounding(self.s(7.0))
                    .build();
            }
            if selected {
                dl.add_rect(p, [p[0] + self.s(3.0), p[1] + item_h], ACCENT).filled(true).build();
            }
            let label = tab.label();
            // Through the same `dl`: imgui-rs allows one window draw list at a
            // time, and a second get_window_draw_list() here panics.
            self.with_font(
                ui,
                |f| f.strong,
                || {
                    let th = ui.calc_text_size(label)[1];
                    dl.add_text([p[0] + self.s(16.0), p[1] + (item_h - th) / 2.0], if selected { FG } else { MUTED }, label);
                },
            );
            let badge = match tab {
                Tab::Invites => pending,
                Tab::Friends => self.data.requests_in.len(),
                _ => 0,
            };
            if badge > 0 {
                let n = badge.to_string();
                let ts = ui.calc_text_size(&n);
                let bw = ts[0] + self.s(12.0);
                let bh = ts[1] + self.s(4.0);
                let bx = p[0] + w - bw - self.s(10.0);
                let by = p[1] + (item_h - bh) / 2.0;
                dl.add_rect([bx, by], [bx + bw, by + bh], ACCENT).filled(true).rounding(bh / 2.0).build();
                dl.add_text([bx + self.s(6.0), by + self.s(2.0)], ON_ACCENT, &n);
            }
        }
    }

    fn panel_footer(&self, ui: &Ui, size: [f32; 2], h: f32) {
        let pos = ui.window_pos();
        let top = size[1] - h;
        ui.get_window_draw_list().add_line([pos[0], pos[1] + top], [pos[0] + size[0], pos[1] + top], LINE).build();
        let line_h = ui.text_line_height() + self.s(4.0);
        ui.set_cursor_pos([self.s(24.0), top + (h - line_h) / 2.0]);
        self.key(ui, "F5");
        ui.same_line();
        ui.text_colored(MUTED, "or");
        ui.same_line();
        self.key(ui, "Esc");
        ui.same_line();
        ui.text_colored(MUTED, "closes.   The game gets no mouse or keys while this is open.");
    }

    fn pane_title(&self, ui: &Ui, title: &str, note: &str) {
        self.heading(ui, title);
        if !note.is_empty() {
            let _c = ui.push_style_color(StyleColor::Text, MUTED);
            ui.text_wrapped(note);
        }
        ui.dummy([0.0, self.s(4.0)]);
    }

    /// A striped row with a title and a muted detail line on the left, and
    /// `action` (buttons `action_w` wide) on the right.
    #[allow(clippy::too_many_arguments)]
    fn row(&self, ui: &Ui, i: usize, dot: Option<[f32; 4]>, title: &str, detail: &str, action_w: f32, action: impl FnOnce()) {
        self.row_colored(ui, i, dot, title, detail, MUTED, action_w, action);
    }

    /// [`Self::row`] with the detail line in `detail_color` (a warning).
    #[allow(clippy::too_many_arguments)]
    fn row_colored(&self, ui: &Ui, i: usize, dot: Option<[f32; 4]>, title: &str, detail: &str, detail_color: [f32; 4], action_w: f32, action: impl FnOnce()) {
        let avail = ui.content_region_avail()[0];
        let start = ui.cursor_screen_pos();
        let local = ui.cursor_pos();
        let pad = self.s(12.0);
        let line = ui.text_line_height();
        let h = line * 2.0 + self.s(22.0);
        // Scoped: `action` may draw too, and only one window draw list can
        // be held at a time.
        let text_x = {
            let dl = ui.get_window_draw_list();
            if i % 2 == 0 {
                dl.add_rect(start, [start[0] + avail, start[1] + h], ROW).filled(true).rounding(self.s(7.0)).build();
            }
            if let Some(color) = dot {
                let r = self.s(4.5);
                dl.add_circle([start[0] + pad + r, start[1] + h / 2.0], r, color).filled(true).build();
                pad + r * 2.0 + self.s(12.0)
            } else {
                pad
            }
        };
        ui.set_cursor_pos([local[0] + text_x, local[1] + self.s(10.0)]);
        self.with_font(ui, |f| f.strong, || ui.text(title));
        ui.set_cursor_pos([local[0] + text_x, local[1] + self.s(12.0) + line]);
        ui.text_colored(detail_color, detail);
        if action_w > 0.0 {
            let button_h = line + self.s(18.0);
            ui.set_cursor_pos([local[0] + avail - pad - action_w, local[1] + (h - button_h) / 2.0]);
            action();
        }
        ui.set_cursor_pos([local[0], local[1] + h + self.s(2.0)]);
        ui.dummy([0.0, 0.0]);
    }

    fn button(&self, ui: &Ui, label: &str, primary: bool) -> bool {
        let _colors = primary.then(|| {
            (
                ui.push_style_color(StyleColor::Button, ACCENT),
                ui.push_style_color(StyleColor::ButtonHovered, ACCENT_HOVER),
                ui.push_style_color(StyleColor::ButtonActive, ACCENT_HOVER),
                ui.push_style_color(StyleColor::Text, ON_ACCENT),
            )
        });
        self.with_font(ui, |f| f.strong, || ui.button(label))
    }

    fn button_width(&self, ui: &Ui, label: &str) -> f32 {
        let visible = label.split("##").next().unwrap_or(label);
        self.with_font(ui, |f| f.strong, || ui.calc_text_size(visible))[0] + self.s(14.0) * 2.0
    }

    /// Buttons on a player's row, right-aligned; runs what was clicked.
    fn player_row(&self, ui: &Ui, i: usize, section: &str, p: &community::Player, detail: &str, actions: &[RowAction]) {
        let labels: Vec<(String, RowAction)> = actions
            .iter()
            .map(|a| {
                let text = match a {
                    RowAction::Invite => "Invite",
                    RowAction::Change(_, text) => text,
                };
                // Ids come from the server: hashed, so none can collide with ImGui's own
                // `##`/`###` markers or another button.
                (format!("{text}##fe-{section}-{:016x}-{text}", ui_id(&p.id)), *a)
            })
            .collect();
        let gap = self.s(8.0);
        #[allow(clippy::cast_precision_loss)]
        let w = labels.iter().map(|(l, _)| self.button_width(ui, l)).sum::<f32>() + gap * labels.len().saturating_sub(1) as f32;
        let dot = if p.online { OK } else { OFFLINE };
        // Someone with a friend's name from another server, who isn't them.
        let (detail, color) = match (&p.lookalike, p.name_status) {
            (Some(server), _) => (format!("Not your friend {} from {server}  ·  {detail}", p.name), BAD),
            (None, community::NameStatus::Conflict) => (format!("{detail}  ·  another player has this name on other servers"), BAD),
            _ => (detail.to_string(), MUTED),
        };
        self.row_colored(ui, i, Some(dot), &p.name, &detail, color, w, || {
            for (n, (label, action)) in labels.iter().enumerate() {
                if n > 0 {
                    ui.same_line_with_spacing(0.0, gap);
                }
                let primary = matches!(action, RowAction::Change(community::FriendChange::Accept, _));
                if self.button(ui, label, primary) {
                    match action {
                        RowAction::Invite => community::invite(p.id.clone(), p.name.clone()),
                        RowAction::Change(change, _) => community::change(*change, p.id.clone(), p.name.clone()),
                    }
                }
            }
        });
    }

    fn status(p: &community::Player) -> String {
        if p.online {
            p.activity.clone().unwrap_or_else(|| String::from("Online, in the menus"))
        } else {
            String::from("Offline")
        }
    }

    fn section(&self, ui: &Ui, title: &str) {
        ui.dummy([0.0, self.s(6.0)]);
        self.with_font(ui, |f| f.strong, || ui.text_colored(MUTED, title));
        ui.dummy([0.0, self.s(2.0)]);
    }

    fn pane_friends(&self, ui: &Ui) {
        use community::FriendChange as C;
        let note = if self.data.everyone_mode {
            "Everyone on this server can invite you. Find players to add friends."
        } else {
            "Only friends can invite you on this server. Find players to add friends."
        };
        self.pane_title(ui, "Friends", note);
        if !self.data.loaded {
            ui.text_colored(MUTED, "Loading friends...");
            return;
        }
        if self.data.my_name_conflict {
            let _c = ui.push_style_color(StyleColor::Text, BAD);
            ui.text_wrapped(format!(
                "Another player has your name on the servers that share friends with this one. Give yourself a new one in {} (Settings, Servers and accounts, Rename).",
                setup_tool()
            ));
            ui.dummy([0.0, self.s(4.0)]);
        }
        let mut row = 0;
        if !self.data.requests_in.is_empty() {
            self.section(ui, "Want to be your friend");
            for p in &self.data.requests_in {
                self.player_row(
                    ui,
                    row,
                    "in",
                    p,
                    &Self::status(p),
                    &[
                        RowAction::Change(C::Block, "Block"),
                        RowAction::Change(C::Decline, "Decline"),
                        RowAction::Change(C::Accept, "Accept"),
                    ],
                );
                row += 1;
            }
        }
        if self.data.friends.is_empty() {
            self.section(ui, "Your friends");
            let _c = ui.push_style_color(StyleColor::Text, MUTED);
            ui.text_wrapped("No friends yet. Open Find players, search for someone and add them.");
        } else {
            self.section(ui, "Your friends");
            for p in &self.data.friends {
                let mut actions = vec![RowAction::Change(C::Block, "Block"), RowAction::Change(C::Remove, "Remove")];
                if p.online {
                    actions.push(RowAction::Invite);
                }
                self.player_row(ui, row, "friend", p, &Self::status(p), &actions);
                row += 1;
            }
        }
        if !self.data.elsewhere.is_empty() {
            self.section(ui, "On other servers");
            {
                let _c = ui.push_style_color(StyleColor::Text, MUTED);
                ui.text_wrapped(format!(
                    "To play together, one of you joins the other's server: {} lists them on its Play screen.",
                    setup_tool()
                ));
            }
            for (name, server) in &self.data.elsewhere {
                self.with_font(ui, |f| f.strong, || ui.text(name));
                ui.same_line();
                ui.text_colored(MUTED, format!("on {server}"));
            }
        }
        if !self.data.requests_out.is_empty() {
            self.section(ui, "Waiting for an answer");
            for p in &self.data.requests_out {
                self.player_row(ui, row, "out", p, "Friend request sent", &[RowAction::Change(C::Decline, "Cancel")]);
                row += 1;
            }
        }
        if !self.data.blocked.is_empty() {
            self.section(ui, "Blocked");
            for p in &self.data.blocked {
                self.player_row(ui, row, "blocked", p, "Can't see you or invite you", &[RowAction::Change(C::Unblock, "Unblock")]);
                row += 1;
            }
        }
    }

    fn pane_players(&mut self, ui: &Ui) {
        use community::FriendChange as C;
        self.pane_title(ui, "Find players", "Search by name, or see who's online. Add friends, or block someone.");
        if !self.searched {
            self.searched = true;
            community::search(self.search_text.trim().to_string());
        }
        let mut text = std::mem::take(&mut self.search_text);
        let search_label = "Search##fe-search-go";
        let field_w = ui.content_region_avail()[0] - self.button_width(ui, search_label) - self.s(10.0);
        ui.set_next_item_width(field_w);
        let entered = ui
            .input_text("##fe-search", &mut text)
            .hint("Name, or empty for everyone online")
            .enter_returns_true(true)
            .build();
        ui.same_line();
        if self.button(ui, search_label, true) || entered {
            community::search(text.trim().to_string());
        }
        text.truncate(32);
        self.search_text = text;
        ui.dummy([0.0, self.s(6.0)]);

        let Some((query, found)) = self.data.search.as_ref() else {
            ui.text_colored(MUTED, "Searching...");
            return;
        };
        if found.is_empty() {
            let _c = ui.push_style_color(StyleColor::Text, MUTED);
            ui.text_wrapped(if query.is_empty() {
                String::from("No one else is online right now.")
            } else {
                format!("No one called \"{query}\".")
            });
            return;
        }
        for (i, p) in found.iter().enumerate() {
            let (detail, mut actions) = match p.relation {
                community::Relation::Friend => (format!("Friend · {}", Self::status(p)), vec![]),
                community::Relation::RequestSent => (String::from("Friend request sent"), vec![RowAction::Change(C::Decline, "Cancel")]),
                community::Relation::RequestReceived => (String::from("Wants to be your friend"), vec![RowAction::Change(C::Accept, "Accept")]),
                community::Relation::Blocked => (String::from("Blocked"), vec![RowAction::Change(C::Unblock, "Unblock")]),
                community::Relation::None => (Self::status(p), vec![RowAction::Change(C::Block, "Block"), RowAction::Change(C::Request, "Add friend")]),
            };
            let detail = match (p.identity.is_empty(), p.name_status) {
                (true, _) => format!("{detail}  ·  no identity"),
                (false, community::NameStatus::Reserved) => format!("{detail}  ·  ID {} (name reserved)", p.identity),
                (false, _) => format!("{detail}  ·  ID {}", p.identity),
            };
            let can_invite = p.online && (p.relation == community::Relation::Friend || (self.data.everyone_mode && p.relation == community::Relation::None));
            if can_invite {
                actions.push(RowAction::Invite);
            }
            self.player_row(ui, i, "found", p, &detail, &actions);
        }
    }

    fn pane_invites(&mut self, ui: &Ui) {
        self.pane_title(ui, "Invites", "Accepting takes you straight into their match.");
        if self.active_invites.is_empty() {
            let _c = ui.push_style_color(StyleColor::Text, MUTED);
            ui.text_wrapped("No invites right now. When someone invites you, it shows up here and in the corner of the screen.");
        }
        let mut accept = None;
        let mut decline = None;
        for (i, invite) in self.active_invites.iter().enumerate() {
            let Some(sender) = invite.event.sender.as_ref() else {
                continue;
            };
            let left = NOTIFICATION_TIMEOUT.saturating_sub(invite.received.elapsed()).as_secs();
            let detail = match self.data.activity_of(&sender.username) {
                Some(a) => format!("{a}  ·  {left} s left"),
                None => format!("{left} s left"),
            };
            let detail = match self.data.lookalike_of(&sender.username) {
                Some(server) => format!("Not your friend {} from {server}  ·  {detail}", sender.username),
                None => detail,
            };
            let accept_label = format!("Accept##fe-acc-{i}");
            let decline_label = format!("Decline##fe-dec-{i}");
            let w = self.button_width(ui, &accept_label) + self.button_width(ui, &decline_label) + self.s(10.0);
            self.row(ui, i, Some(OK), &sender.username, &detail, w, || {
                if self.button(ui, &decline_label, false) {
                    decline = Some(i);
                }
                ui.same_line();
                if self.button(ui, &accept_label, true) {
                    accept = Some(i);
                }
            });
        }
        if let Some(i) = accept {
            if let Some(sender) = self.active_invites[i].event.sender.clone() {
                let _ = self.tx.send(invite_accept_event(sender.id));
            }
            self.active_invites[i].clicked = true;
            self.invite_notification = None;
            self.ui_state = UiState::Hide;
        } else if let Some(i) = decline {
            self.active_invites.remove(i);
            self.invite_notification = None;
        }
        ui.dummy([0.0, self.s(8.0)]);
        let auto = hooks_config::get().is_some_and(|c| c.auto_join_invite);
        ui.text_colored(
            MUTED,
            if auto {
                "Joining invites automatically is on (AutoJoinInvite in uplay.toml)."
            } else {
                "Joining invites automatically is off (AutoJoinInvite in uplay.toml)."
            },
        );
    }

    /// The lobby's player counts, which the game only has while you're in a
    /// lobby (the fields live in its game session).
    fn pane_match(&self, ui: &Ui) {
        let min = unsafe { get_min_players_var().as_mut() };
        let max = unsafe { get_max_players_var().as_mut() };
        let (Some(min), Some(max)) = (min, max) else {
            self.pane_title(ui, "Match", "Host or join a lobby to change these.");
            let _c = ui.push_style_color(StyleColor::Text, MUTED);
            ui.text_wrapped("While you're in a lobby: the players needed to start the match, and the most players allowed in it.");
            return;
        };
        self.pane_title(ui, "Match", "Settings for the lobby you're in. Change them before the match starts.");
        let most = *max;
        self.stepper(ui, 0, "Players needed to start", "The match waits for this many players", min, 1, most.max(1));
        let least = (*min).max(1);
        self.stepper(ui, 1, "Most players", "Spies vs Mercs classic is 2 against 2, co-op is 2", max, least, 99);
    }

    #[allow(clippy::too_many_arguments)]
    fn stepper(&self, ui: &Ui, i: usize, title: &str, detail: &str, value: &mut i32, lo: i32, hi: i32) {
        let minus = format!("-##fe-step-{i}-dn");
        let plus = format!("+##fe-step-{i}-up");
        let num_w = self.s(44.0);
        let bw = self.button_width(ui, &plus);
        let w = bw * 2.0 + num_w + self.s(20.0);
        self.row(ui, i, None, title, detail, w, || {
            if self.button(ui, &minus, false) && *value > lo {
                *value -= 1;
            }
            ui.same_line();
            let text = value.to_string();
            let tw = self.with_font(ui, |f| f.strong, || ui.calc_text_size(&text))[0];
            let cur = ui.cursor_pos();
            ui.set_cursor_pos([cur[0] + (num_w - tw) / 2.0, cur[1] + self.s(9.0)]);
            self.with_font(ui, |f| f.strong, || ui.text(&text));
            ui.set_cursor_pos([cur[0] + num_w + self.s(10.0), cur[1]]);
            if self.button(ui, &plus, false) && *value < hi {
                *value += 1;
            }
        });
    }

    fn pane_server(&mut self, ui: &Ui) {
        self.pane_title(ui, "Server", "");
        let host = hooks_config::get().and_then(|c| c.api_server.host_str().map(String::from)).unwrap_or_default();
        let server = match &self.data.server {
            Some(info) if !info.revision.is_empty() => format!("{} {} ({})", info.name, info.version, info.revision),
            Some(info) => format!("{} {}", info.name, info.version),
            None => String::from("Not reported"),
        };
        let rows = [
            ("Address", host),
            ("Signed in as", crate::api::username().unwrap_or_else(|| String::from("Not signed in"))),
            (
                "Response time",
                self.data.response_time.map_or_else(|| String::from("No answer"), |d| format!("{} ms", d.as_millis())),
            ),
            ("Server", server),
            ("Other players reach you", crate::hooks::nat::status().unwrap_or_else(|| String::from("Not started yet"))),
            (
                "Your identity",
                if self.data.my_identity.is_empty() {
                    String::from("Not linked")
                } else {
                    self.data.my_identity.clone()
                },
            ),
            ("Game add-on", format!("{PRODUCT} {RELEASE}")),
        ];
        let x = ui.cursor_pos()[0];
        let label_w = self.s(170.0);
        for (label, value) in rows {
            ui.text_colored(MUTED, label);
            ui.same_line_with_pos(x + label_w);
            ui.text(value);
        }
        ui.dummy([0.0, self.s(10.0)]);
        let busy = self.relogin.is_some();
        let label = if busy { "Signing in...##fe-relogin" } else { "Sign in again##fe-relogin" };
        let mut clicked = false;
        ui.disabled(busy, || clicked = self.button(ui, label, false));
        if clicked && !busy {
            self.relogin = std::thread::Builder::new()
                .name(String::from("overlay-relogin"))
                .spawn(|| crate::api::runtime().map(|rt| rt.block_on(crate::api::relogin())).unwrap_or(false))
                .ok();
        }
    }

    /// The developer window (OverlayDebug = true in uplay.toml).
    fn show_debug(&mut self, ui: &Ui) {
        ui.window("Developer").always_auto_resize(true).build(|| {
            ui.input_text("username", &mut self.username).build();
            if ui.button("Friend Accepted Invite") {
                info!("Send friend invite accept for {}", self.username);
                let _ = self.tx.send(Event::FriendsGameInviteAccepted(self.username.clone()));
            }
            ui.same_line();
            if ui.button("Party Accepted Invite") {
                info!("Send party invite accept for {}", self.username);
                let _ = self.tx.send(Event::PartyGameInviteAccepted(self.username.clone()));
            }
        });
    }
}

fn get_game_settings() -> *mut i32 {
    if let Some(ncaddr) = unsafe { crate::hooks::NET_CORE_ADDR } {
        unsafe {
            // let g_netcore = std::ptr::read(0x32b_5dc4 as *mut *mut *mut i32);
            let g_netcore = ncaddr as *mut *mut i32;
            if g_netcore.is_null() {
                return std::ptr::null_mut();
            }
            let game_session = std::ptr::read(g_netcore.byte_add(0x5d0));
            if game_session.is_null() {
                return std::ptr::null_mut();
            }
            game_session.byte_add(0x594)
        }
    } else {
        std::ptr::null_mut()
    }
}

fn get_max_players_var() -> *mut i32 {
    unsafe {
        let game_settings = get_game_settings();
        if game_settings.is_null() {
            return std::ptr::null_mut();
        }

        game_settings.byte_add(0x20)
    }
}

fn get_min_players_var() -> *mut i32 {
    unsafe {
        let game_settings = get_game_settings();
        if game_settings.is_null() {
            return std::ptr::null_mut();
        }

        game_settings.byte_add(0x1c)
    }
}

impl ImguiRenderLoop for MyRenderLoop {
    fn initialize(&mut self, ctx: &mut imgui::Context, _render_context: &mut dyn hudhook::RenderContext) {
        theme_colors(ctx.style_mut());
        self.fonts = Some(add_fonts(ctx));
        // Nothing to remember between games; don't write imgui.ini.
        ctx.set_ini_filename(None::<std::path::PathBuf>);
    }

    fn before_render(&mut self, ctx: &mut imgui::Context, _render_context: &mut dyn hudhook::RenderContext) {
        let height = ctx.io().display_size[1];
        self.s = if height > 0.0 { (height / DESIGN_HEIGHT).clamp(0.6, 3.0) } else { 1.0 };
        ctx.io_mut().font_global_scale = self.s / FONT_OVERSAMPLE;
        theme_sizes(ctx.style_mut(), self.s);
    }

    fn render(&mut self, ui: &mut imgui::Ui) {
        self.poll_keys();
        self.poll_invites();
        self.poll_relogin();
        self.data = community::snapshot();

        if self.ui_state == UiState::Show {
            if self.last_refresh.elapsed() >= PANEL_REFRESH {
                community::refresh();
                self.last_refresh = Instant::now();
            }
            self.show_panel(ui);
            if hooks_config::get().is_some_and(|c| c.overlay_debug) {
                self.show_debug(ui);
            }
        }
        self.show_error_banner(ui);
        self.show_invite_toast(ui);
        self.show_initial_info(ui);
        self.show_notice(ui);
    }

    fn message_filter(&self, _io: &imgui::Io) -> hudhook::MessageFilter {
        if matches!(self.ui_state, UiState::Show) {
            hudhook::MessageFilter::InputAll
        } else {
            hudhook::MessageFilter::empty()
        }
    }

    fn on_wnd_proc(&self, hwnd: windows::Win32::Foundation::HWND, umsg: u32, wparam: windows::Win32::Foundation::WPARAM, lparam: windows::Win32::Foundation::LPARAM) {
        if matches!(self.ui_state, UiState::Show) {
            // Forward to default handler so that the window doesn't break
            unsafe {
                DefWindowProcA(hwnd, umsg, wparam, lparam);
            }
        }
    }
}

fn init_hudhook<T: hudhook::Hooks + 'static>(invites: crossbeam_channel::Receiver<Result<Option<InviteEvent>, crate::api::Error>>) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel();
    EVENTS.get_or_init(|| Mutex::new(rx));
    crate::uplay_r1_loader::EVENT_SENDER.get_or_init(|| Mutex::new(tx.clone()));
    community::start();
    hudhook::Hudhook::builder()
        .with::<T>(MyRenderLoop {
            tx,
            username: String::from("ABCD"),
            ui_state: UiState::default(),
            tab: Tab::default(),
            f5_down: false,
            esc_down: false,
            invite_notification: None,
            new_invites: invites,
            active_invites: Vec::new(),
            connection_error: None,
            initial_popup: Instant::now() + INITIAL_POPUP_DURATION,
            fonts: None,
            s: 1.0,
            data: community::Snapshot::default(),
            local_notice: None,
            relogin: None,
            last_refresh: Instant::now(),
            search_text: String::new(),
            searched: false,
        })
        .with_hmodule(unsafe { GetModuleHandleA(PCSTR::null())?.into() })
        .build()
        .apply()
        .map_err(|e| anyhow::anyhow!("Error adding gui hook: {e:?}"))
}

fn init_dx9(invites: crossbeam_channel::Receiver<Result<Option<InviteEvent>, crate::api::Error>>) -> anyhow::Result<()> {
    init_hudhook::<hudhook::hooks::dx9::ImguiDx9Hooks>(invites)
}

fn init_dx11(invites: crossbeam_channel::Receiver<Result<Option<InviteEvent>, crate::api::Error>>) -> anyhow::Result<()> {
    init_hudhook::<hudhook::hooks::dx11::ImguiDx11Hooks>(invites)
}

pub fn init(engine: Engine, invites: crossbeam_channel::Receiver<Result<Option<InviteEvent>, crate::api::Error>>) -> anyhow::Result<()> {
    match engine {
        Engine::DX9 => init_dx9(invites),
        Engine::DX11 => init_dx11(invites),
    }
}
