//! The window and the core between them: what the screens show, read on worker threads from
//! core.rs and handed to the window in its own thread; what the player presses, sent back the
//! other way. The window never waits on the network or the game's files.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use launcher::prefs::PlayMode;
use launcher::prefs::Prefs;
use slint::ComponentHandle;
use slint::Model;
use slint::ModelRc;
use slint::SharedString;
use slint::VecModel;

use crate::core;
use crate::core::Changed;
use crate::core::Core;
use crate::tauri::Manager;
use crate::AppWindow;

/// What the window shows and needs to remember between looks.
#[derive(Default)]
struct Seen {
    info: core::Info,
    checks: Option<core::Checklist>,
    notice: Option<core::NoticeView>,
    notice_at: Option<Instant>,
    all_checks: bool,
    /// What the Play button does now.
    press: Press,
    screen: String,
    lobbies: launcher::matches::Lobbies,
    lobbies_loaded: bool,
    filter: Filter,
    picked: Option<u32>,
    /// The match watched for a seat, and its label.
    watching: Option<(u32, String)>,
    seat_open: Option<(u32, String)>,
    started: Option<Instant>,
}

/// The Matches screen's filters.
struct Filter {
    /// "all", "svm" or "coop".
    mode: String,
    under_way: bool,
    full: bool,
    friends_first: bool,
    /// 0 nearest first, 1 most players, 2 newest.
    sort: i32,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            mode: "all".into(),
            under_way: true,
            full: false,
            friends_first: true,
            sort: 0,
        }
    }
}

/// What pressing the Play button does.
#[derive(Debug, Clone, Default, PartialEq)]
enum Press {
    #[default]
    Nothing,
    Play,
    Fix(&'static str),
    FixEverything,
    /// The setup: the trial has none yet, so it says where to go.
    Setup,
}

/// The window's half: the core, the window, and what's been seen.
pub struct Ui {
    core: Arc<Core>,
    window: slint::Weak<AppWindow>,
    seen: Mutex<Seen>,
    /// Looks running now, so a slow one isn't asked for again while it runs; and those asked
    /// for again meanwhile (they run once more when it ends).
    running: Mutex<std::collections::HashSet<&'static str>>,
    again: Mutex<std::collections::HashSet<&'static str>>,
    quit: AtomicBool,
    /// The app the command modules live in (shell.rs), the pages and dialogs, and what's done
    /// on them, in order, on one thread of its own.
    app: std::sync::OnceLock<crate::tauri::AppHandle>,
    pub(crate) pages: crate::shell::Pages,
    actions: Mutex<Option<std::sync::mpsc::Sender<Box<dyn FnOnce(&Arc<Ui>) + Send>>>>,
    /// The system's own scaling, before the player's size.
    base_scale: Mutex<f32>,
}

impl Ui {
    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Runs `f` on a worker thread, unless the look called `name` is running already.
    fn spawn(self: &Arc<Self>, name: &'static str, f: impl FnOnce(&Arc<Ui>) + Send + 'static) {
        if !self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(name) {
            return;
        }
        let ui = self.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                f(&ui);
                ui.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name);
            })
            .ok();
    }

    /// Runs `f` on a worker thread; asked for again while it runs, it runs once more after.
    pub(crate) fn spawn_named(self: &Arc<Self>, name: &'static str, f: impl Fn(&Arc<Ui>) + Send + Sync + 'static) {
        {
            let mut running = self.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if running.contains(name) {
                self.again.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(name);
                return;
            }
            running.insert(name);
        }
        let ui = self.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || loop {
                f(&ui);
                let mut running = ui.running.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if ui.again.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name) {
                    continue;
                }
                running.remove(name);
                break;
            })
            .ok();
    }

    /// Something the player did on a page or a dialog: done in order, on the actions thread.
    pub(crate) fn act(self: &Arc<Self>, f: impl FnOnce(&Arc<Ui>) + Send + 'static) {
        if let Some(tx) = self.actions.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            let _ = tx.send(Box::new(f));
        }
    }

    /// Runs `f` on the window's thread with the window, if it's still open.
    fn show(&self, f: impl FnOnce(&AppWindow) + Send + 'static) {
        let _ = self.window.upgrade_in_event_loop(move |w| f(&w));
    }

    pub(crate) fn window(&self) -> &slint::Weak<AppWindow> {
        &self.window
    }

    pub(crate) fn app(&self) -> crate::tauri::AppHandle {
        self.app.get().cloned().expect("the app is made before the window runs")
    }

    pub(crate) fn screen(&self) -> String {
        self.seen().screen.clone()
    }

    pub(crate) fn base_scale(&self) -> f32 {
        *self.base_scale.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// To a screen ("settings:network" opens Settings at Network). Play is the guided setup
    /// while that's needed.
    pub(crate) fn go(self: &Arc<Self>, to: &str) {
        let (screen, section) = to.split_once(':').unwrap_or((to, ""));
        let mut screen = screen.to_string();
        if screen == "play" && self.setup_shown() {
            screen = "setup".into();
        }
        if !section.is_empty() {
            let section = section.to_string();
            self.with_page(&screen, |p| p.section(&section));
        }
        self.seen().screen = screen.clone();
        let shown = screen.clone();
        self.show(move |w| w.set_screen(s(shown)));
        match screen.as_str() {
            "matches" => self.load_lobbies(),
            "play" => self.reload_play(),
            _ => self.load_page(),
        }
    }

    /// Whether the guided setup shows instead of Play.
    fn setup_shown(&self) -> bool {
        let info = self.seen().info.clone();
        self.pages.setup.lock().unwrap_or_else(std::sync::PoisonError::into_inner).shown(&info)
    }
}

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

fn ms(ping: Option<u32>) -> i32 {
    ping.map_or(-1, |p| p.min(i32::MAX as u32) as i32)
}

fn mode_id(mode: PlayMode) -> &'static str {
    match mode {
        PlayMode::Play => "play",
        PlayMode::Svm => "svm",
        PlayMode::Coop => "coop",
    }
}

fn mode_from(id: &str) -> PlayMode {
    match id {
        "svm" => PlayMode::Svm,
        "coop" => PlayMode::Coop,
        _ => PlayMode::Play,
    }
}

fn mode_label(mode: PlayMode) -> &'static str {
    match mode {
        PlayMode::Play => "Play",
        PlayMode::Svm => "Play SvM",
        PlayMode::Coop => "Play Co-op",
    }
}

/// The Play button: whether the player can play and what pressing it does (the 0.5.0 plan's
/// P2), as the Tauri launcher's Play.vue works it out. Its tone is said by its words too, never
/// by the colour alone. With something to fix, the button is the fix; playing anyway stays in the
/// modes menu.
fn play_button(info: &core::Info, list: Option<&core::Checklist>, notice: Option<&core::NoticeView>) -> (crate::PlayButton, Press) {
    let mode = mode_label(info.play_mode);
    let button = |label: &str, sub: &str, tone: &str, disabled: bool, busy: bool| crate::PlayButton {
        label: s(label),
        sub: s(sub),
        tone: s(tone),
        disabled,
        busy,
        why: s(""),
        action: s(""),
    };
    let failing: Vec<&core::CheckView> = list.map(|l| l.checks.iter().filter(|c| c.status == "fail").collect()).unwrap_or_default();
    if info.running {
        return (button("In game", "", "ok", true, false), Press::Nothing);
    }
    if info.game_dir.is_none() {
        let find = failing.iter().find_map(|c| c.fix);
        let mut b = button("Find the game", "Game not found", "muted", find.is_none(), false);
        b.why = s("Choose the game's folder first.");
        return (b, find.map_or(Press::Nothing, |(id, _)| Press::Fix(id)));
    }
    if info.server.is_none() {
        return (button("Choose a server", "No server yet", "muted", false, false), Press::Setup);
    }
    if let Some(blocks) = notice.and_then(|n| n.blocks_play.as_ref()) {
        let mut b = button("Server updating", &notice.map(|n| n.status.clone()).unwrap_or_default(), "muted", true, false);
        b.why = s(blocks);
        return (b, Press::Nothing);
    }
    let new_match = if info.new_match { "New match · " } else { "" };
    let Some(list) = list else {
        return (button(mode, &format!("{new_match}Checking…"), "checking", true, true), Press::Nothing);
    };
    if let Some(preparing) = info.preparing {
        return (button(mode, &format!("{new_match}{preparing}…"), "checking", true, true), Press::Nothing);
    }
    if list.tone == Some("fail") {
        let fixes: Vec<(&'static str, &'static str)> = failing.iter().filter_map(|c| c.fix).collect();
        let sub = list.line.as_str();
        if list.busy {
            return (button("Fixing…", sub, "fail", true, true), Press::Nothing);
        }
        if let [(id, label)] = fixes[..] {
            let press = if matches!(id, "choose-server" | "set-up-account") {
                Press::Setup
            } else {
                Press::Fix(id)
            };
            return (button(label, sub, "fail", false, false), press);
        }
        if fixes.len() > 1 && list.fixable {
            return (button(&format!("Fix {} things", fixes.len()), sub, "fail", false, false), Press::FixEverything);
        }
        let mut b = button("See what to fix", sub, "fail", false, false);
        b.action = s("status");
        return (b, Press::Nothing);
    }
    if list.tone == Some("warn") || notice.is_some() {
        let line = notice.map_or_else(|| list.line.clone(), |n| n.status.clone());
        return (button(mode, &format!("{new_match}{line}"), "warn", false, false), Press::Play);
    }
    let sub = if info.new_match {
        "A new match".to_string()
    } else if list.line.is_empty() {
        "Ready to play".to_string()
    } else {
        list.line.clone()
    };
    (button(mode, &sub, "ok", false, false), Press::Play)
}

/// The first letter of a name, for an avatar.
fn initial(name: &str) -> String {
    name.chars().next().map_or_else(|| "?".to_string(), |c| c.to_uppercase().collect())
}

impl Ui {
    /// The window's InfoView, and the Play button from what's been seen.
    fn push_info(self: &Arc<Self>) {
        let mut seen = self.seen();
        let (button, press) = play_button(&seen.info, seen.checks.as_ref(), seen.notice.as_ref());
        seen.press = press;
        let info = seen.info.clone();
        let busy = seen.checks.as_ref().is_some_and(|c| c.busy);
        drop(seen);
        let view = crate::InfoView {
            game_dir: s(info.game_dir.clone().unwrap_or_default()),
            server: s(info.server.clone().unwrap_or_default()),
            account: s(info.account.clone().unwrap_or_default()),
            identity: s(info.identity.clone().unwrap_or_default()),
            community: info.community,
            play_mode: s(mode_id(info.play_mode)),
            managed: info.managed,
            running: info.running,
            connecting: info.connecting,
            beta: info.beta as i32,
            name_not_allowed: s(info.name_not_allowed.clone().unwrap_or_default()),
            emulator_notice: info.emulator_notice,
            new_match: info.new_match,
            requests: info.requests as i32,
        };
        // Why the server can't be changed now, if it can't.
        let server_blocked = if info.running {
            "Quit the game first"
        } else if info.connecting || busy {
            "Wait for the current task to finish"
        } else if info.managed {
            "Another tool manages this install: change the server there"
        } else {
            ""
        };
        // Playing anyway, while the button is a fix, stays in the modes menu.
        let modes_allowed = !info.running && !info.managed && info.game_dir.is_some() && info.server.is_some() && !button.busy && button.label != "Server updating";
        let matches_ready = info.game_dir.is_some() && info.server.is_some() && !info.running;
        let account = info.account.clone().unwrap_or_default();
        self.show(move |w| {
            w.set_info(view);
            w.set_button(button);
            w.set_server_blocked(s(server_blocked));
            w.set_modes_allowed(modes_allowed);
            w.set_matches_ready(matches_ready);
            w.set_initial(s(if account.is_empty() { "?".into() } else { initial(&account) }));
        });
    }

    pub fn load_info(self: &Arc<Self>) {
        self.spawn("info", |ui| {
            let info = core::info(&ui.core);
            ui.seen().info = info;
            ui.push_info();
            // The guided setup comes and goes with what's set up.
            let screen = ui.screen();
            let setup = ui.setup_shown();
            if (screen == "play" && setup) || (screen == "setup" && !setup) {
                ui.go("play");
            }
        });
    }

    /// The activity bar, read on a thread of its own every time (cheap, and never skipped: the
    /// core's notifier may call this while it holds its lock).
    pub fn load_activity(self: &Arc<Self>) {
        let ui = self.clone();
        std::thread::spawn(move || ui.push_activity());
    }

    fn push_activity(self: &Arc<Self>) {
        let a = core::activity(&self.core);
        self.show(move |w| w.set_activity(activity_view(a)));
    }

    pub fn load_checks(self: &Arc<Self>, fresh: bool) {
        self.spawn("checks", move |ui| {
            let all = ui.seen().all_checks;
            let list = core::checks(&ui.core, fresh, all);
            let checks: Vec<crate::CheckRow> = list
                .checks
                .iter()
                .map(|c| crate::CheckRow {
                    id: s(c.id),
                    status: s(c.status),
                    title: s(&c.title),
                    detail: s(&c.detail),
                    fix_id: s(c.fix.map_or("", |f| f.0)),
                    fix_label: s(c.fix.map_or("", |f| f.1)),
                })
                .collect();
            let (support_status, support_title, support_why) = list.support.clone().unwrap_or_default();
            let (fixable, busy, trouble) = (list.fixable, list.busy, list.tone == Some("fail"));
            let signed_in = list.signed_in;
            // The server refused this launcher as outdated: the update banner says so.
            static TOLD: AtomicBool = AtomicBool::new(false);
            let outdated = list.checks.iter().any(|c| c.fix.is_some_and(|f| f.0 == "update-launcher"));
            if outdated && !TOLD.swap(true, Ordering::Relaxed) {
                crate::update::outdated(&ui.app());
            } else if !outdated {
                TOLD.store(false, Ordering::Relaxed);
            }
            {
                let mut seen = ui.seen();
                seen.checks = Some(list);
            }
            ui.push_info();
            ui.show(move |w| {
                w.set_checks(model(checks));
                w.set_support_status(s(support_status));
                w.set_support_title(s(support_title));
                w.set_support_why(s(support_why));
                w.set_fixable(fixable);
                w.set_checks_busy(busy);
                w.set_trouble(trouble);
                w.set_signed_in(signed_in);
                w.set_checks_loaded(true);
                w.set_all_checks(all);
                w.set_loaded(true);
            });
        });
    }

    pub fn load_servers(self: &Arc<Self>, fresh: bool) {
        self.show(|w| w.set_pinging(true));
        self.spawn("servers", move |ui| {
            let got = core::servers(&ui.core, fresh);
            let rows: Vec<crate::ServerRow> = got
                .rows
                .iter()
                .map(|r| crate::ServerRow {
                    host: s(&r.host),
                    place: s(&r.place),
                    name: s(&r.name),
                    ping: ms(r.ping),
                    players: r.players as i32,
                    playing: s(&r.playing),
                    current: r.current,
                    best: r.best,
                    maintenance: s(r.maintenance.clone().unwrap_or_default()),
                })
                .collect();
            let card = got.card.map_or_else(crate::ServerCard::default, |c| crate::ServerCard {
                shown: true,
                caption: s(c.caption),
                caption_why: s(c.caption_why.unwrap_or_default()),
                place: s(c.place),
                ping: ms(c.ping),
                listed: c.players.is_some(),
                players: c.players.unwrap_or(0) as i32,
            });
            let (header, note, network) = (got.header, got.note.unwrap_or_default(), got.network.is_some());
            ui.show(move |w| {
                w.set_servers(model(rows));
                w.set_card(card);
                w.set_servers_header(s(header));
                w.set_servers_note(s(note));
                w.set_network(network);
                w.set_pinging(false);
            });
        });
    }

    pub fn load_squad(self: &Arc<Self>, fresh: bool) {
        self.spawn("squad", move |ui| {
            let q = core::squad(&ui.core, fresh);
            let online: Vec<crate::FriendRow> = q
                .online
                .iter()
                .map(|f| crate::FriendRow {
                    id: s(&f.id),
                    name: s(&f.name),
                    activity: s(f.activity.clone().unwrap_or_default()),
                    joinable: f.joinable,
                    with_me: f.with_me,
                })
                .collect();
            let others_total = if q.managed { 0 } else { q.others.len() as i32 };
            let others: Vec<crate::OtherRow> = q
                .others
                .iter()
                .filter(|_| !q.managed)
                .take(8)
                .map(|o| crate::OtherRow {
                    id: s(&o.id),
                    name: s(&o.name),
                    activity: s(o.activity.clone().unwrap_or_default()),
                    asked: o.asked,
                    asked_us: o.asked_us,
                })
                .collect();
            let elsewhere: Vec<crate::ElsewhereRow> = q
                .elsewhere
                .iter()
                .map(|e| crate::ElsewhereRow {
                    name: s(&e.name),
                    place: s(&e.place),
                    host: s(&e.host),
                    public: e.public,
                })
                .collect();
            let playing_shown = q.playing.is_some();
            let playing: Vec<SharedString> = q.playing.clone().unwrap_or_default().into_iter().map(SharedString::from).collect();
            let more = q.playing_more.clone().unwrap_or_default();
            ui.show(move |w| {
                w.set_online(model(online));
                w.set_others(model(others));
                w.set_others_total(others_total);
                w.set_elsewhere(model(elsewhere));
                w.set_offline(q.offline as i32);
                w.set_squad_requests(q.requests as i32);
                w.set_join_offered(q.join_offered);
                w.set_squad_managed(q.managed);
                w.set_playing_shown(playing_shown);
                w.set_playing(model(playing));
                w.set_playing_more(s(more));
                w.set_squad_loaded(true);
            });
        });
    }

    pub fn load_news(self: &Arc<Self>) {
        self.spawn("news", |ui| {
            let news: Vec<SharedString> = core::news(&ui.core).into_iter().map(SharedString::from).collect();
            ui.show(move |w| w.set_news(model(news)));
        });
    }

    pub fn load_notice(self: &Arc<Self>) {
        self.spawn("notice", |ui| {
            let n = core::notice(&ui.core);
            let shown = n.clone();
            {
                let mut seen = ui.seen();
                seen.notice_at = Some(Instant::now() + n.as_ref().map_or(Duration::from_secs(60), |n| n.every));
                seen.notice = n;
            }
            ui.push_info();
            ui.show(move |w| w.set_notice(notice_view(shown)));
        });
    }

    /// Once: whether the server offers picking a match, the modes, the game's art, and a move.
    pub fn load_once(self: &Arc<Self>) {
        self.spawn("once", |ui| {
            let offered = core::choose_match_offered(&ui.core);
            let svm: Vec<crate::ModeItem> = core::svm_modes()
                .into_iter()
                .map(|(playlist, name)| crate::ModeItem {
                    playlist: playlist as i32,
                    name: s(name),
                    coop: false,
                })
                .collect();
            ui.show(move |w| {
                w.set_choose_offered(offered);
                w.set_svm_modes(model(svm));
            });
            if let Some((rgba, width, height)) = core::key_art(&ui.core) {
                if rgba.len() == (width * height * 4) as usize {
                    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, width, height);
                    ui.show(move |w| {
                        w.set_art(slint::Image::from_rgba8(buffer));
                        w.set_has_art(true);
                    });
                }
            }
            if core::follow_move(&ui.core).is_some() {
                ui.load_info();
            }
        });
    }

    pub fn load_lobbies(self: &Arc<Self>) {
        self.spawn("lobbies", |ui| {
            let got = core::lobbies(&ui.core);
            {
                let mut seen = ui.seen();
                seen.lobbies = got;
                seen.lobbies_loaded = true;
            }
            ui.push_lobbies();
            ui.check_watched();
        });
    }

    /// The matches as filtered and sorted, and the picked one in full.
    fn push_lobbies(self: &Arc<Self>) {
        let mut guard = self.seen();
        let seen = &mut *guard;
        let f = &seen.filter;
        let all = &seen.lobbies.lobbies;
        let coop = |l: &launcher::matches::Lobby| l.kind == "coop";
        let (count_svm, count_coop) = (all.iter().filter(|l| !coop(l)).count(), all.iter().filter(|l| coop(l)).count());
        let picked = seen.picked;
        let mut shown: Vec<&launcher::matches::Lobby> = all
            .iter()
            .filter(|l| {
                (f.mode == "all" || (f.mode == "coop") == coop(l))
                    && (f.under_way || !l.under_way)
                    // A full match under way is the Under way filter's.
                    && (f.full || !l.full || l.under_way || Some(l.room) == picked)
            })
            .collect();
        let key = |l: &launcher::matches::Lobby| -> i64 {
            match f.sort {
                1 => -(l.players as i64),
                2 => l.open_secs as i64,
                _ => l.ping_ms.map_or(9999, i64::from),
            }
        };
        let friends_first = f.friends_first;
        shown.sort_by(|a, b| {
            let friends = if friends_first {
                (!b.friends.is_empty()).cmp(&!a.friends.is_empty())
            } else {
                std::cmp::Ordering::Equal
            };
            friends.then_with(|| key(a).cmp(&key(b)))
        });
        // The first one picked once the list comes, and again when the picked one goes.
        let picked = if shown.iter().any(|l| Some(l.room) == picked) {
            picked
        } else {
            shown.first().map(|l| l.room)
        };
        let selected: Option<launcher::matches::Lobby> = shown.iter().find(|l| Some(l.room) == picked).map(|l| (*l).clone());
        let shown: Vec<launcher::matches::Lobby> = shown.into_iter().cloned().collect();
        seen.picked = picked;
        let f = &seen.filter;
        let (mode, under_way, full, friends, sort) = (f.mode.clone(), f.under_way, f.full, f.friends_first, f.sort);
        let watching = seen.watching.as_ref().map_or(-1, |(room, _)| *room as i32);
        let (loaded, error, summary, hidden, total) = (
            seen.lobbies_loaded,
            seen.lobbies.error.clone().unwrap_or_default(),
            seen.lobbies.summary.clone(),
            seen.lobbies.hidden.clone(),
            all.len() as i32,
        );
        drop(guard);
        self.show(move |w| {
            w.set_lobbies_loaded(loaded);
            w.set_lobbies_error(s(error));
            w.set_lobbies_summary(s(summary));
            w.set_lobbies_hidden(s(hidden));
            w.set_lobbies_total(total);
            w.set_count_svm(count_svm as i32);
            w.set_count_coop(count_coop as i32);
            w.set_filter_mode(s(mode));
            w.set_show_under_way(under_way);
            w.set_show_full(full);
            w.set_friends_first(friends);
            w.set_sort(sort);
            w.set_has_selected(selected.is_some());
            w.set_selected(selected.as_ref().map(lobby_row).unwrap_or_default());
            let rows: Vec<crate::LobbyRow> = shown.iter().map(lobby_row).collect();
            // Rows already shown are changed in place: the list keeps its scroll and focus.
            set_rows(&w.get_shown(), rows, |rc| w.set_shown(rc));
            w.set_watching_room(watching);
        });
    }

    /// A watched match with a seat now: the line with Join, and the window asks for attention.
    fn check_watched(self: &Arc<Self>) {
        let mut seen = self.seen();
        let Some((room, label)) = seen.watching.clone() else { return };
        match seen.lobbies.lobbies.iter().find(|l| l.room == room) {
            None => seen.watching = None,
            Some(l) if l.joinable => {
                seen.watching = None;
                seen.seat_open = Some((room, label.clone()));
                drop(seen);
                self.show(move |w| {
                    w.set_seat_open(s(label));
                    w.set_watching_room(-1);
                    use slint::winit_030::WinitWindowAccessor as _;
                    w.window()
                        .with_winit_window(|win| win.request_user_attention(Some(slint::winit_030::winit::window::UserAttentionType::Informational)));
                });
            }
            Some(_) => {}
        }
    }

    /// Everything on Play asked again (a setup or a fix ended).
    pub(crate) fn reload_play(self: &Arc<Self>) {
        self.load_info();
        self.load_checks(false);
        self.load_servers(false);
        self.load_squad(false);
        self.load_news();
    }
}

/// The activity bar's view (built in the window's thread: its list is a model).
fn activity_view(a: Option<core::ActivityView>) -> crate::ActivityView {
    match a {
        None => crate::ActivityView::default(),
        Some(a) => crate::ActivityView {
            shown: true,
            id: a.id as i32,
            title: s(a.title),
            step: s(a.step.unwrap_or_default()),
            state: s(a.state),
            message: s(a.message.unwrap_or_default()),
            progress: a.progress.unwrap_or(-1.0),
            actions: model(a.actions.into_iter().map(|(id, label)| crate::ActionItem { id: s(id), label: s(label) }).collect()),
            waiting: a.waiting as i32,
        },
    }
}

/// The notice's view (built in the window's thread).
fn notice_view(n: Option<core::NoticeView>) -> crate::NoticeView {
    let Some(n) = n else { return crate::NoticeView::default() };
    crate::NoticeView {
        shown: true,
        heading: s(&n.heading),
        eta: s(&n.eta),
        text: s(&n.text),
        note: s(n.note.clone().unwrap_or_default()),
        play_on_host: s(n.play_on.as_ref().map_or("", |p| p.0.as_str())),
        play_on_label: s(n.play_on.as_ref().map_or("", |p| p.1.as_str())),
        servers: model(
            n.servers
                .iter()
                .map(|r| crate::NoticeRow {
                    place: s(&r.place),
                    mine: r.mine,
                    version: s(&r.version),
                    doing: s(&r.doing),
                    meter: r.meter.unwrap_or(-1.0),
                    tone: s(r.tone),
                })
                .collect(),
        ),
    }
}

/// `rows` into the model shown: changed in place where the count is the same.
fn set_rows(current: &ModelRc<crate::LobbyRow>, rows: Vec<crate::LobbyRow>, replace: impl FnOnce(ModelRc<crate::LobbyRow>)) {
    if let Some(vec) = current.as_any().downcast_ref::<VecModel<crate::LobbyRow>>() {
        if vec.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if vec.row_data(i).as_ref() != Some(&row) {
                    vec.set_row_data(i, row);
                }
            }
            return;
        }
        vec.set_vec(rows);
        return;
    }
    replace(model(rows));
}

fn person(p: &launcher::matches::Person, l: &launcher::matches::Lobby) -> crate::PersonRow {
    let party_line = if p.party == 0 {
        String::new()
    } else {
        let others: Vec<&str> = l.people.iter().filter(|q| q.party == p.party && q.name != p.name).map(|q| q.name.as_str()).collect();
        if others.is_empty() {
            "in a party".into()
        } else {
            format!("party with {}", others.join(", "))
        }
    };
    crate::PersonRow {
        name: s(&p.name),
        initial: s(initial(&p.name)),
        host: p.host,
        party: p.party as i32,
        party_line: s(party_line),
        friend: p.friend,
        team: s(&p.team),
        ping: ms(p.ping_ms),
        me: p.me,
    }
}

/// A match as the list and the details show it.
fn lobby_row(l: &launcher::matches::Lobby) -> crate::LobbyRow {
    // A friend hosting is said once, in "Hosted by"; the line names the others.
    let others: Vec<&String> = l.friends.iter().filter(|f| **f != l.host).collect();
    let mut line = Vec::new();
    if !l.host.is_empty() {
        line.push(if l.friends.contains(&l.host) {
            format!("Hosted by {}, your friend", l.host)
        } else {
            format!("Hosted by {}", l.host)
        });
    }
    match others.as_slice() {
        [] => {}
        [one] => line.push(format!("{one}, your friend, is in it")),
        [first, rest @ ..] => line.push(format!("{first} and {} more of your friends are in it", rest.len())),
    }
    if let Some(made_for) = l.made_for {
        line.push(format!("{} seats (made for {made_for})", l.seats));
    }
    let people: Vec<crate::PersonRow> = l.people.iter().map(|p| person(p, l)).collect();
    let side = |team: &str| -> Vec<crate::PersonRow> { l.people.iter().filter(|p| p.team == team).map(|p| person(p, l)).collect() };
    crate::LobbyRow {
        room: l.room as i32,
        mode: s(&l.mode),
        kind: s(&l.kind),
        map: s(l.map.clone().unwrap_or_default()),
        players: l.players as i32,
        seats: l.seats as i32,
        made_for: l.made_for.map_or(-1, |m| m as i32),
        host: s(&l.host),
        status: s(&l.status),
        tone: s(l.tone),
        under_way: l.under_way,
        full: l.full,
        joinable: l.joinable,
        open_for: s(&l.open_for),
        ping: ms(l.ping_ms),
        line: s(line.join(" · ")),
        has_friends: !l.friends.is_empty(),
        teams: l.teams,
        people: model(people),
        spies: model(side("spy")),
        mercs: model(side("merc")),
        with_me: l.with_me,
        settings: model(l.settings.iter().map(|(k, v)| crate::SettingRow { key: s(k), value: s(v) }).collect()),
    }
}

/// Builds the window, joins it to the core and runs it until it closes.
pub fn run() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    window.set_release(s(env!("FE_RELEASE")));
    let theme = match Prefs::theme() {
        launcher::prefs::Theme::Dark => crate::ThemeChoice::Dark,
        launcher::prefs::Theme::Light => crate::ThemeChoice::Light,
        launcher::prefs::Theme::System => crate::ThemeChoice::System,
    };
    window.global::<crate::Theme>().set_choice(theme);
    window.global::<crate::Theme>().set_reduced_motion(std::env::var_os("FE_REDUCED_MOTION").is_some());

    // The core tells the window what changed; the window asks again.
    let ui_slot: Arc<Mutex<Option<std::sync::Weak<Ui>>>> = Arc::new(Mutex::new(None));
    let upgrade = {
        let slot = ui_slot.clone();
        move || slot.lock().ok().and_then(|s| s.as_ref().and_then(std::sync::Weak::upgrade))
    };
    let notify: core::Notify = {
        let upgrade = upgrade.clone();
        Arc::new(move |what| {
            let Some(ui) = upgrade() else { return };
            match what {
                Changed::Activity => ui.load_activity(),
                Changed::Checks => {
                    ui.reload_play();
                    if ui.screen() == "setup" {
                        ui.load_page();
                    }
                }
                Changed::NeedsName => ui.needs_name(),
            }
        })
    };
    let core = Arc::new(Core::new(notify));
    // A first setup: no game or no server yet when the launcher starts (its options come last).
    let info = core::info(&core);
    let fresh = !info.managed && (info.game_dir.is_none() || info.server.is_none());
    let ui = Arc::new(Ui {
        core: core.clone(),
        window: window.as_weak(),
        seen: Mutex::new(Seen {
            screen: "play".into(),
            started: Some(Instant::now()),
            info,
            ..Seen::default()
        }),
        running: Mutex::new(Default::default()),
        again: Mutex::new(Default::default()),
        quit: AtomicBool::new(false),
        app: std::sync::OnceLock::new(),
        pages: crate::shell::Pages::new(fresh),
        actions: Mutex::new(None),
        base_scale: Mutex::new(window.window().scale_factor()),
    });
    *ui_slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&ui));

    // The app the command modules live in: their states, their events, the window.
    let events: crate::tauri::EventSink = {
        let upgrade = upgrade.clone();
        Arc::new(move |name: &str| {
            if let Some(ui) = upgrade() {
                ui.event(name);
            }
        })
    };
    let hooks: crate::tauri::WindowHooks = {
        let upgrade = upgrade.clone();
        Arc::new(move |call| upgrade().is_some_and(|ui| ui.window_call(call)))
    };
    let app = crate::tauri::Builder::new()
        .manage(crate::commands::Launcher(core))
        .manage(crate::feedback::Feedback::default())
        .manage(crate::whats_new::WhatsNew::default())
        .manage(crate::intel::Intel::default())
        .manage(crate::support::Support::default())
        .manage(crate::chat::Chat::default())
        .manage(crate::host::Host::default())
        .manage(crate::update::Update::default())
        .build(events, hooks);
    let _ = ui.app.set(app.clone());

    // What's done on the pages and dialogs, in order.
    let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce(&Arc<Ui>) + Send>>();
    *ui.actions.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
    {
        let ui = ui.clone();
        std::thread::Builder::new()
            .name("actions".into())
            .spawn(move || {
                while let Ok(f) = rx.recv() {
                    let _ = launcher::task::guarded(|| {
                        f(&ui);
                        Ok::<_, String>(())
                    });
                }
            })
            .ok();
    }

    wire(&window, &ui);
    crate::shell::wire(&window, &ui);
    crate::feedback::start(&app);
    crate::support::start(&app);
    crate::chat::start(&app);
    crate::update::start(&app);
    told_once(&ui);
    // The player's size for the launcher, kept from before.
    let size = Prefs::ui_scale();
    if (size - 1.0).abs() > 0.01 {
        // Once the window is up with its own scale (the system's), the player's on top.
        let ui2 = ui.clone();
        slint::Timer::single_shot(Duration::from_millis(400), move || {
            if let Some(w) = ui2.window().upgrade() {
                *ui2.base_scale.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = w.window().scale_factor();
            }
            ui2.apply_size(size);
        });
    }
    if ui.setup_shown() {
        ui.go("play");
    }

    // Gamescope (the Deck's gaming mode): full screen, no window buttons.
    if gamescope() {
        window.set_full_screen_mode(true);
        window.window().set_fullscreen(true);
    }

    ui.load_info();
    ui.load_activity();
    ui.load_checks(false);
    ui.load_servers(false);
    ui.load_squad(false);
    ui.load_news();
    ui.load_notice();
    ui.load_once();
    ui.load_banner();
    ui.check_dialogs(true);
    start_ticker(&ui);

    let result = window.run();
    ui.quit.store(true, Ordering::Relaxed);
    // A server started here stops with the launcher.
    ui.app().state::<crate::host::Host>().stop();
    result
}

/// At start, once: a player who agreed to the diagnostics before 0.4.3 is told what they now
/// include (not asked again).
fn told_once(ui: &Arc<Ui>) {
    let mut inner = ui.core.lock();
    if inner.game.as_ref().is_some_and(|g| g.cfg.hook_config.send_diagnostics) && Prefs::tell_log_excerpt() {
        inner.notices.info(
            "New in the game's diagnostics: when something goes wrong, the server can ask for the 15 minutes of the game's log before it (at most once an hour, never the whole log, private details hidden). Settings › Feedback turns the diagnostics off.",
        );
    }
}

impl Ui {
    /// A setup stopped for a name: the setup's name step when it shows, else the account dialog.
    fn needs_name(self: &Arc<Self>) {
        let Some((server, suggested, taken)) = self.core.lock().needs_name.take() else { return };
        if self.screen() == "setup" {
            self.pages
                .setup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .needs_name(server, suggested, taken);
            self.load_page();
        } else {
            self.open_dialog(crate::pages::dialogs::Dialog::Account(crate::pages::dialogs::Account::new(server, suggested, taken, false)));
        }
    }
}

/// Whether the launcher runs inside gamescope (the Steam Deck's gaming mode), as the egui
/// launcher tells (scale.rs).
fn gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some() || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("gamescope"))
}

/// Asks again on its own: the game and the activity bar often, the rest as each needs.
fn start_ticker(ui: &Arc<Ui>) {
    let ui = ui.clone();
    std::thread::Builder::new()
        .name("ticker".into())
        .spawn(move || {
            let mut tick: u64 = 0;
            while !ui.quit.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                tick += 1;
                let (preparing, screen, notice_due, watching, started) = {
                    let seen = ui.seen();
                    (
                        seen.info.preparing.is_some(),
                        seen.screen.clone(),
                        seen.notice_at.is_some_and(|at| Instant::now() >= at),
                        seen.watching.is_some(),
                        seen.started,
                    )
                };
                // The splash never stays up on a slow check: the screens say "Checking…".
                if started.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
                    ui.show(|w| w.set_loaded(true));
                }
                if preparing || tick % 6 == 0 {
                    ui.load_info();
                }
                if tick % 2 == 0 {
                    ui.load_activity();
                }
                if tick % 60 == 0 {
                    ui.load_checks(false);
                }
                if tick % 30 == 0 {
                    ui.load_squad(false);
                }
                if notice_due {
                    ui.load_notice();
                }
                if screen == "matches" && tick % 10 == 0 || watching && tick % 20 == 0 {
                    ui.load_lobbies();
                }
                // The page shown, as often as it asks; chat's dot every few seconds.
                let every = ui.with_page(&screen, |p| p.refresh_every()).unwrap_or(0);
                let due = ui.pages.refreshed.lock().map(|t| t.elapsed() >= Duration::from_secs(every)).unwrap_or(false);
                if every > 0 && due {
                    ui.load_page();
                }
                if tick % 6 == 3 {
                    ui.load_glance();
                    ui.load_banner();
                }
                if tick % 2 == 1 {
                    let ui2 = ui.clone();
                    ui.show(move |w| {
                        use slint::winit_030::WinitWindowAccessor as _;
                        let focused = w.window().with_winit_window(|win| win.has_focus()).unwrap_or(true);
                        ui2.pages.focused.store(focused, Ordering::Relaxed);
                    });
                }
            }
        })
        .ok();
}

/// The window's callbacks.
fn wire(w: &AppWindow, ui: &Arc<Ui>) {
    use slint::winit_030::WinitWindowAccessor as _;
    let weak = w.as_weak();

    w.on_drag({
        let weak = weak.clone();
        move || {
            if let Some(w) = weak.upgrade() {
                w.window().with_winit_window(|win| {
                    let _ = win.drag_window();
                });
            }
        }
    });
    w.on_minimise({
        let weak = weak.clone();
        move || {
            if let Some(w) = weak.upgrade() {
                w.window().set_minimized(true);
            }
        }
    });
    w.on_maximise({
        let weak = weak.clone();
        move || {
            if let Some(w) = weak.upgrade() {
                let to = !w.window().is_maximized();
                w.window().set_maximized(to);
                w.set_maximised(to);
            }
        }
    });
    w.on_close_window({
        let weak = weak.clone();
        let ui = ui.clone();
        move || {
            // A report still sending: wait for it, or close and send it next time.
            let app = ui.app();
            if crate::feedback::report_sending(app.state::<crate::feedback::Feedback>()) {
                ui.open_dialog(crate::pages::dialogs::Dialog::Closing { waiting: false });
                return;
            }
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        }
    });
    w.on_cycle_theme({
        let weak = weak.clone();
        move || {
            let Some(w) = weak.upgrade() else { return };
            let theme = w.global::<crate::Theme>();
            let (next, keep) = match theme.get_choice() {
                crate::ThemeChoice::Dark => (crate::ThemeChoice::Light, launcher::prefs::Theme::Light),
                crate::ThemeChoice::Light => (crate::ThemeChoice::System, launcher::prefs::Theme::System),
                crate::ThemeChoice::System => (crate::ThemeChoice::Dark, launcher::prefs::Theme::Dark),
            };
            theme.set_choice(next);
            std::thread::spawn(move || Prefs::set_theme(keep));
        }
    });
    w.on_move_focus({
        let weak = weak.clone();
        move |forward| {
            let Some(w) = weak.upgrade() else { return };
            use slint::platform::Key;
            use slint::platform::WindowEvent;
            let key: SharedString = if forward { Key::Tab.into() } else { Key::Backtab.into() };
            w.window().dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
            w.window().dispatch_event(WindowEvent::KeyReleased { text: key });
        }
    });
    w.on_go({
        let ui = ui.clone();
        move |screen| ui.go(&screen)
    });
    w.on_whats_new({
        let ui = ui.clone();
        move || {
            if let Some(view) = crate::whats_new::whats_new_again() {
                ui.open_dialog(crate::pages::dialogs::Dialog::WhatsNew {
                    view: crate::pages::json(&view),
                    on_start: false,
                    closed: false,
                });
            }
        }
    });
    w.on_act({
        let ui = ui.clone();
        let weak = weak.clone();
        move |id, action| {
            let ui2 = ui.clone();
            let _ = &weak;
            std::thread::spawn(move || {
                // A report, a suggestion or a support message that couldn't be sent: theirs.
                let app = ui2.app();
                let pressed = match action.as_str() {
                    "retry" => Some(launcher::activity::Action::Retry),
                    "without-logs" => Some(launcher::activity::Action::WithoutLogs),
                    _ => None,
                };
                if let Some(pressed) = pressed {
                    let id = id as u64;
                    if crate::feedback::take_action(&app, id, pressed) || crate::intel::take_action(&app, id, pressed) || crate::support::take_action(&app, id, pressed) {
                        ui2.core.lock().notices.press(id, pressed);
                        ui2.load_activity();
                        return;
                    }
                }
                if let Some(text) = core::activity_action(&ui2.core, id as u64, &action) {
                    ui2.show(move |w| w.invoke_copy_text(s(text)));
                }
                ui2.load_activity();
            });
        }
    });
    w.on_dismiss({
        let ui = ui.clone();
        move |id| {
            let ui = ui.clone();
            std::thread::spawn(move || core::dismiss(&ui.core, id as u64));
        }
    });

    // Play.
    w.on_play_pressed({
        let ui = ui.clone();
        let weak = weak.clone();
        move || {
            let press = ui.seen().press.clone();
            match press {
                Press::Nothing => {}
                Press::Play => {
                    let mode = ui.seen().info.play_mode;
                    ui.spawn("play", move |ui| {
                        core::play(&ui.core, mode);
                        ui.check_dialogs(false);
                        ui.load_info();
                    });
                }
                Press::Fix(id) => ui.spawn("fix", move |ui| fix_or_setup(ui, id)),
                Press::FixEverything => ui.spawn("fix", |ui| {
                    core::fix_everything(&ui.core);
                }),
                Press::Setup => {
                    ui.pages.setup.lock().unwrap_or_else(std::sync::PoisonError::into_inner).editing = true;
                    let _ = &weak;
                    ui.go("play");
                }
            }
        }
    });
    w.on_choose_mode({
        let ui = ui.clone();
        move |mode| {
            let mode = mode_from(&mode);
            ui.seen().info.play_mode = mode;
            ui.spawn("play", move |ui| {
                Prefs::set_play_mode(mode);
                core::play(&ui.core, mode);
                ui.check_dialogs(false);
                ui.load_info();
            });
            ui.push_info();
        }
    });
    w.on_cancel_new_match({
        let ui = ui.clone();
        move || {
            ui.spawn("cancel-new-match", |ui| {
                core::cancel_new_match(&ui.core);
                ui.load_info();
            })
        }
    });
    w.on_load_servers({
        let ui = ui.clone();
        move |fresh| ui.load_servers(fresh)
    });
    w.on_switch_server({
        let ui = ui.clone();
        move |host| {
            ui.spawn("switch", move |ui| {
                if let Err(why) = core::switch_server(&ui.core, &host) {
                    ui.core.lock().notices.info(why);
                    ui.load_activity();
                }
                ui.load_info();
            })
        }
    });
    w.on_fix({
        let ui = ui.clone();
        move |id| ui.spawn("fix", move |ui| fix_or_setup(ui, &id))
    });
    w.on_fix_everything({
        let ui = ui.clone();
        move || {
            ui.spawn("fix", |ui| {
                core::fix_everything(&ui.core);
            })
        }
    });
    w.on_identify({
        let ui = ui.clone();
        move || ui.spawn("fix", |ui| core::identify(&ui.core))
    });
    w.on_check_again({
        let ui = ui.clone();
        move || {
            ui.show(|w| w.set_checks_busy(true));
            ui.load_checks(true);
        }
    });
    w.on_toggle_all({
        let ui = ui.clone();
        move || {
            {
                let mut seen = ui.seen();
                seen.all_checks = !seen.all_checks;
            }
            ui.load_checks(false);
        }
    });
    w.on_join_friend({
        let ui = ui.clone();
        move |id, name| {
            ui.spawn("join", move |ui| {
                core::join_friend(&ui.core, &id, &name);
                ui.check_dialogs(false);
                ui.load_info();
            })
        }
    });
    w.on_player_action({
        let ui = ui.clone();
        move |action, id, name| {
            ui.spawn("player-action", move |ui| {
                core::player_action(&ui.core, &action, &id, &name);
                ui.load_squad(true);
            })
        }
    });
    w.on_switch_to_friend({
        let ui = ui.clone();
        move |host| {
            ui.spawn("switch", move |ui| {
                core::switch_to_friend(&ui.core, &host);
            })
        }
    });
    w.on_dismiss_emulator({
        let ui = ui.clone();
        move || ui.spawn("emulator", |ui| core::dismiss_emulator_notice(&ui.core))
    });
    w.on_ask_review({
        let ui = ui.clone();
        move || {
            let name = ui.seen().info.name_not_allowed.clone().unwrap_or_default();
            let app = ui.app();
            crate::support::draft(
                &app,
                format!("Please review my name {name}: I was asked to choose another, and I think that's a mistake. What it means: "),
            );
            ui.go("support");
        }
    });

    // Matches.
    let filter = |ui: &Arc<Ui>, f: Box<dyn Fn(&mut Filter)>| {
        f(&mut ui.seen().filter);
        ui.push_lobbies();
    };
    w.on_set_filter({
        let ui = ui.clone();
        move |m| filter(&ui, Box::new(move |f| f.mode = m.to_string()))
    });
    w.on_set_under_way({
        let ui = ui.clone();
        move |on| filter(&ui, Box::new(move |f| f.under_way = on))
    });
    w.on_set_full({
        let ui = ui.clone();
        move |on| filter(&ui, Box::new(move |f| f.full = on))
    });
    w.on_set_friends_first({
        let ui = ui.clone();
        move |on| filter(&ui, Box::new(move |f| f.friends_first = on))
    });
    w.on_set_sort({
        let ui = ui.clone();
        move |i| filter(&ui, Box::new(move |f| f.sort = i))
    });
    w.on_pick({
        let ui = ui.clone();
        move |room| {
            ui.seen().picked = Some(room as u32);
            ui.push_lobbies();
        }
    });
    w.on_join({
        let ui = ui.clone();
        move |room| {
            ui.show(|w| w.set_joining(true));
            ui.spawn("join", move |ui| {
                core::join_match(&ui.core, room as u32);
                ui.check_dialogs(false);
                ui.show(|w| w.set_joining(false));
                ui.load_info();
            })
        }
    });
    w.on_host({
        let ui = ui.clone();
        move |m| {
            let (mode, playlist) = if m.coop { (PlayMode::Coop, None) } else { (PlayMode::Svm, Some(m.playlist as u32)) };
            ui.spawn("play", move |ui| {
                core::new_match(&ui.core, mode, playlist);
                ui.check_dialogs(false);
                ui.load_info();
            })
        }
    });
    w.on_quick({
        let ui = ui.clone();
        move |m| {
            let (mode, playlist) = if m.coop { (PlayMode::Coop, None) } else { (PlayMode::Svm, Some(m.playlist as u32)) };
            ui.spawn("play", move |ui| {
                core::quick_match(&ui.core, mode, playlist);
                ui.check_dialogs(false);
                ui.load_info();
            })
        }
    });
    w.on_watch({
        let ui = ui.clone();
        move |room, label| {
            {
                let mut seen = ui.seen();
                let room = room as u32;
                seen.watching = if seen.watching.as_ref().is_some_and(|(r, _)| *r == room) {
                    None
                } else {
                    Some((room, label.to_string()))
                };
                seen.seat_open = None;
            }
            ui.show(|w| w.set_seat_open(s("")));
            ui.push_lobbies();
        }
    });
    w.on_join_watched({
        let ui = ui.clone();
        move || {
            let seat = ui.seen().seat_open.take();
            ui.show(|w| w.set_seat_open(s("")));
            if let Some((room, _)) = seat {
                ui.spawn("join", move |ui| {
                    core::join_match(&ui.core, room);
                    ui.check_dialogs(false);
                    ui.load_info();
                });
            }
        }
    });
    w.on_dismiss_seat({
        let ui = ui.clone();
        move || {
            ui.seen().seat_open = None;
            ui.show(|w| w.set_seat_open(s("")));
        }
    });
    w.on_retry_lobbies({
        let ui = ui.clone();
        move || ui.load_lobbies()
    });
}

/// A check's fix: the launcher's own, the update, or the setup's server step (choosing a
/// server, setting up the account).
fn fix_or_setup(ui: &Arc<Ui>, id: &str) {
    if id == "update-launcher" {
        let app = ui.app();
        crate::update::update_asked(app.clone(), app.state::<crate::update::Update>());
        return;
    }
    if !core::fix(&ui.core, id) {
        ui.pages.setup.lock().unwrap_or_else(std::sync::PoisonError::into_inner).editing = true;
        ui.go("play");
    }
}
