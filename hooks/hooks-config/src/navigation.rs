//! What the quick start does in the game's online menus, as plain decisions: given what the menu
//! shows this tick, wait, select an entry, or give up. The game's DLL reads the menu
//! (`hooks::quickstart`) and does the selecting; everything it decides is here, so every mode and
//! every way a menu can look is tested without the game.
//!
//! The steps, each waiting for the menu to show its entry:
//! 1. the lobby: the online menu's Spies vs Mercs (id 1) or Co-op (id 2) entry;
//! 2. for the launcher's New match or Quick match: Quick match (3) or, for Co-op, Find teammate
//!    (5);
//! 3. Spies vs Mercs only: the mode, in the list Quick match opens (its entries' ids are the
//!    playlists): the one the player chose, else the one the game has focused.
//!
//! An entry is selected only once it has stood in an unchanged list for [`STABLE_TICKS`] ticks
//! (the menus fill in over a few frames). A step after the lobby that isn't done in
//! [`STEP_GIVE_UP`] is given up (the player is somewhere else), and Esc stops it all
//! ([`Navigator::abort`]); either way the game is the player's again.

use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

/// The online menu's entries for the lobbies.
pub const ITEM_SVM: u32 = 1;
pub const ITEM_COOP: u32 = 2;
/// Spies vs Mercs' menu: Quick match and Private match.
pub const ITEM_QUICK_MATCH: u32 = 3;
pub const ITEM_PRIVATE_MATCH: u32 = 4;
/// Co-op's menu: Find teammate.
pub const ITEM_FIND_TEAMMATE: u32 = 5;
/// An empty slot in a menu's list.
pub const ITEM_NONE: u32 = u32::MAX;

/// The Spies vs Mercs playlists (the mode list's entry ids), as `game_names::PLAYLISTS` has them
/// (a test in the launcher keeps the two the same): a list with one of these is the mode list.
pub const SVM_PLAYLISTS: &[u32] = &[4_008_726_265, 2_165_463_540, 709_003_443, 655_282_787, 4_078_930_099, 1_487_234_637, 1_365_420_058];

/// How many ticks an entry must stand in an unchanged list before it's selected.
pub const STABLE_TICKS: u32 = 30;
/// A step after the lobby not done in this long is given up.
pub const STEP_GIVE_UP: Duration = Duration::from_secs(60);

/// What the menu shows this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    /// The entries' ids on the current page ([`ITEM_NONE`] for an empty slot).
    pub ids: Vec<u32>,
    /// The menu takes input (not mid-transition).
    pub active: bool,
    /// The list's page (the online menu is page 0).
    pub page: u32,
    /// The entry the game has focused, if it says.
    pub focus: Option<u32>,
}

/// A step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    /// The online menu's Spies vs Mercs or Co-op entry.
    Lobby { coop: bool },
    /// Quick match (Spies vs Mercs) or Find teammate (Co-op).
    Matchmaking { coop: bool },
    /// The mode: this playlist, else the focused one.
    Mode(Option<u32>),
}

impl Goal {
    /// How long the step may take; the lobby has none here (the quick start's own five minutes).
    fn give_up_after(self) -> Option<Duration> {
        match self {
            Goal::Lobby { .. } => None,
            Goal::Matchmaking { .. } | Goal::Mode(_) => Some(STEP_GIVE_UP),
        }
    }
}

/// What to do this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do (no steps left).
    Idle,
    /// Not yet: the entry isn't there, or hasn't stood still long enough.
    Wait,
    /// Focus entry `pos` (id `id`) and select it; the step is done. `fallback`: the chosen mode
    /// wasn't in the list, so the focused one is taken.
    Select { goal: Goal, pos: usize, id: u32, fallback: bool },
    /// The step took too long: every step left is dropped.
    GiveUp(Goal),
}

/// The steps left and how far the current one is.
#[derive(Debug, Default)]
pub struct Navigator {
    steps: VecDeque<Goal>,
    /// Ticks the current step's entry has stood in an unchanged list.
    seen: u32,
    /// That list, as last seen.
    last_ids: Vec<u32>,
    /// When the current step started.
    since: Option<Instant>,
}

impl Navigator {
    /// To the lobby of Spies vs Mercs or Co-op.
    pub fn to_lobby(coop: bool) -> Self {
        let mut n = Self::default();
        n.steps.push_back(Goal::Lobby { coop });
        n
    }

    /// Adds matchmaking after the steps there are: Quick match (or Find teammate), and for
    /// Spies vs Mercs the mode (`playlist`, else the focused one).
    pub fn then_matchmaking(&mut self, coop: bool, playlist: Option<u32>) {
        self.steps.push_back(Goal::Matchmaking { coop });
        if !coop {
            self.steps.push_back(Goal::Mode(playlist));
        }
    }

    /// The step under way.
    pub fn current(&self) -> Option<Goal> {
        self.steps.front().copied()
    }

    /// Steps are left.
    pub fn busy(&self) -> bool {
        !self.steps.is_empty()
    }

    /// Stops it all (the player pressed Esc, or the quick start was given up): `true` if
    /// anything was left to do.
    pub fn abort(&mut self) -> bool {
        let busy = self.busy();
        self.steps.clear();
        self.restart(None);
        busy
    }

    fn restart(&mut self, now: Option<Instant>) {
        self.seen = 0;
        self.last_ids.clear();
        self.since = now;
    }

    /// This tick: `view` is what the menu shows (`None`: it can't be read).
    pub fn tick(&mut self, now: Instant, view: Option<&MenuView>) -> Action {
        let Some(goal) = self.current() else { return Action::Idle };
        let since = *self.since.get_or_insert(now);
        if goal.give_up_after().is_some_and(|limit| now.saturating_duration_since(since) >= limit) {
            self.abort();
            return Action::GiveUp(goal);
        }
        let Some(view) = view else {
            self.seen = 0;
            return Action::Wait;
        };
        let Some((pos, fallback)) = pick(goal, view) else {
            self.seen = 0;
            self.last_ids.clear();
            return Action::Wait;
        };
        if view.ids != self.last_ids {
            self.last_ids.clone_from(&view.ids);
            self.seen = 0;
        }
        self.seen += 1;
        if self.seen < STABLE_TICKS {
            return Action::Wait;
        }
        self.steps.pop_front();
        self.restart(self.busy().then_some(now));
        Action::Select {
            goal,
            pos,
            id: view.ids[pos],
            fallback,
        }
    }
}

/// The entry for `goal` in what the menu shows, if it's the right list and ready: its position,
/// and whether it's the focused mode taken in place of a chosen one that isn't there.
pub fn pick(goal: Goal, view: &MenuView) -> Option<(usize, bool)> {
    if !view.active {
        return None;
    }
    let find = |id: u32| view.ids.iter().position(|&i| i == id);
    match goal {
        Goal::Lobby { coop } => {
            if view.page != 0 {
                return None;
            }
            find(if coop { ITEM_COOP } else { ITEM_SVM }).map(|p| (p, false))
        }
        Goal::Matchmaking { coop } => find(if coop { ITEM_FIND_TEAMMATE } else { ITEM_QUICK_MATCH }).map(|p| (p, false)),
        Goal::Mode(chosen) => {
            if !is_mode_list(&view.ids) {
                return None;
            }
            if let Some(p) = chosen.and_then(find) {
                return Some((p, false));
            }
            let focused = view
                .focus
                .map(|f| f as usize)
                .filter(|&f| view.ids.get(f).is_some_and(|&id| id != ITEM_NONE))
                .or_else(|| view.ids.iter().position(|&id| id != ITEM_NONE))?;
            Some((focused, chosen.is_some()))
        }
    }
}

/// The list Quick match opens: it has a Spies vs Mercs playlist in it.
pub fn is_mode_list(ids: &[u32]) -> bool {
    ids.iter().any(|id| SVM_PLAYLISTS.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACKLIST: u32 = 4_008_726_265;
    const CLASSIC: u32 = 2_165_463_540;
    const TDM: u32 = 709_003_443;
    const EXTRACTION: u32 = 655_282_787;
    const UPLINK: u32 = 4_078_930_099;
    /// The modes a new match can be hosted in (`game_names::svm_new_match_modes`).
    const MODES: [u32; 5] = [BLACKLIST, CLASSIC, TDM, EXTRACTION, UPLINK];

    /// The menus as the game showed them (gamebox logs, 2026-11-05/06).
    fn online_menu() -> MenuView {
        view(&[ITEM_SVM, ITEM_COOP, 7, ITEM_NONE, ITEM_NONE, ITEM_NONE], 0, Some(0))
    }
    fn svm_menu() -> MenuView {
        view(&[ITEM_QUICK_MATCH, ITEM_PRIVATE_MATCH], 1, Some(0))
    }
    fn coop_menu() -> MenuView {
        view(&[ITEM_FIND_TEAMMATE, 6], 1, Some(0))
    }
    fn mode_list() -> MenuView {
        // The game's order on gb1: Classic first (focused), then Blacklist and the rest.
        view(&[CLASSIC, BLACKLIST, EXTRACTION, UPLINK, TDM], 2, Some(0))
    }
    fn view(ids: &[u32], page: u32, focus: Option<u32>) -> MenuView {
        MenuView {
            ids: ids.to_vec(),
            active: true,
            page,
            focus,
        }
    }

    /// Ticks `n` frames 16 ms apart from `t` with the same view; the last action, and the time.
    fn run(nav: &mut Navigator, t: Instant, n: u32, v: Option<&MenuView>) -> (Action, Instant) {
        let mut action = Action::Wait;
        let mut now = t;
        for _ in 0..n {
            action = nav.tick(now, v);
            if matches!(action, Action::Select { .. } | Action::GiveUp(_) | Action::Idle) {
                break;
            }
            now += Duration::from_millis(16);
        }
        (action, now)
    }

    /// Ticks until something other than Wait (at most `limit` ticks).
    fn until_done(nav: &mut Navigator, t: Instant, v: &MenuView) -> (Action, Instant) {
        run(nav, t, STABLE_TICKS * 4, Some(v))
    }

    #[test]
    fn to_the_lobby_of_each_kind() {
        for coop in [false, true] {
            let mut nav = Navigator::to_lobby(coop);
            let (a, _) = until_done(&mut nav, Instant::now(), &online_menu());
            let want = if coop { ITEM_COOP } else { ITEM_SVM };
            assert_eq!(
                a,
                Action::Select {
                    goal: Goal::Lobby { coop },
                    pos: (want - 1) as usize,
                    id: want,
                    fallback: false
                }
            );
            assert!(!nav.busy());
        }
    }

    #[test]
    fn new_or_quick_match_in_every_spies_vs_mercs_mode() {
        for mode in MODES {
            let t = Instant::now();
            let mut nav = Navigator::to_lobby(false);
            nav.then_matchmaking(false, Some(mode));
            let (a, t) = until_done(&mut nav, t, &online_menu());
            assert!(matches!(a, Action::Select { id: ITEM_SVM, .. }), "{mode}: {a:?}");
            let (a, t) = until_done(&mut nav, t, &svm_menu());
            assert!(matches!(a, Action::Select { id: ITEM_QUICK_MATCH, .. }), "{mode}: {a:?}");
            let (a, t) = until_done(&mut nav, t, &mode_list());
            let pos = mode_list().ids.iter().position(|&i| i == mode).unwrap();
            assert_eq!(
                a,
                Action::Select {
                    goal: Goal::Mode(Some(mode)),
                    pos,
                    id: mode,
                    fallback: false
                }
            );
            assert!(!nav.busy());
            assert_eq!(nav.tick(t, Some(&mode_list())), Action::Idle, "nothing more once done");
        }
    }

    #[test]
    fn co_op_matchmaking_is_find_teammate_with_no_mode_step() {
        let t = Instant::now();
        let mut nav = Navigator::to_lobby(true);
        nav.then_matchmaking(true, Some(BLACKLIST)); // a stray playlist changes nothing
        let (_, t) = until_done(&mut nav, t, &online_menu());
        let (a, _) = until_done(&mut nav, t, &coop_menu());
        assert!(
            matches!(
                a,
                Action::Select {
                    id: ITEM_FIND_TEAMMATE,
                    pos: 0,
                    ..
                }
            ),
            "{a:?}"
        );
        assert!(!nav.busy(), "Co-op has no mode list");
    }

    #[test]
    fn no_mode_chosen_takes_the_one_the_game_has_focused() {
        let mut nav = Navigator::default();
        nav.then_matchmaking(false, None);
        let (_, t) = until_done(&mut nav, Instant::now(), &svm_menu());
        let mut list = mode_list();
        list.focus = Some(3);
        let (a, _) = until_done(&mut nav, t, &list);
        assert_eq!(
            a,
            Action::Select {
                goal: Goal::Mode(None),
                pos: 3,
                id: UPLINK,
                fallback: false
            }
        );
    }

    #[test]
    fn a_chosen_mode_missing_from_the_list_falls_back_to_the_focused_one() {
        let mut nav = Navigator::default();
        nav.then_matchmaking(false, Some(BLACKLIST));
        let (_, t) = until_done(&mut nav, Instant::now(), &svm_menu());
        let list = view(&[CLASSIC, TDM], 2, Some(1));
        let (a, _) = until_done(&mut nav, t, &list);
        assert_eq!(
            a,
            Action::Select {
                goal: Goal::Mode(Some(BLACKLIST)),
                pos: 1,
                id: TDM,
                fallback: true
            }
        );
    }

    #[test]
    fn a_focus_out_of_range_or_on_an_empty_slot_takes_the_first_real_entry() {
        for focus in [None, Some(9), Some(0)] {
            let list = view(&[ITEM_NONE, CLASSIC, BLACKLIST], 2, focus);
            assert_eq!(pick(Goal::Mode(None), &list), Some((1, false)), "focus {focus:?}");
        }
    }

    #[test]
    fn the_mode_step_never_selects_in_another_list() {
        // The online menu (ids 1, 2, 7: none of them 3 or 4, which the old check looked for),
        // Spies vs Mercs' menu still showing, Co-op's menu, an empty list.
        for other in [online_menu(), svm_menu(), coop_menu(), view(&[], 2, None), view(&[ITEM_NONE; 4], 2, Some(0))] {
            for chosen in [None, Some(BLACKLIST)] {
                assert_eq!(pick(Goal::Mode(chosen), &other), None, "{other:?}");
            }
        }
        // And the other steps don't select in the mode list.
        for goal in [Goal::Lobby { coop: false }, Goal::Matchmaking { coop: false }, Goal::Matchmaking { coop: true }] {
            assert_eq!(pick(goal, &mode_list()), None, "{goal:?}");
        }
    }

    #[test]
    fn the_lobby_is_only_taken_on_the_online_menus_first_page() {
        let mut other_page = online_menu();
        other_page.page = 1;
        assert_eq!(pick(Goal::Lobby { coop: false }, &other_page), None);
    }

    #[test]
    fn nothing_is_selected_while_the_menu_is_inactive_or_unreadable() {
        let mut nav = Navigator::to_lobby(false);
        let mut inactive = online_menu();
        inactive.active = false;
        let (a, t) = run(&mut nav, Instant::now(), STABLE_TICKS * 3, Some(&inactive));
        assert_eq!(a, Action::Wait);
        let (a, t) = run(&mut nav, t, STABLE_TICKS * 3, None);
        assert_eq!(a, Action::Wait);
        // Ready now: it still waits the full count from here, not from before.
        let (a, _) = run(&mut nav, t, STABLE_TICKS - 1, Some(&online_menu()));
        assert_eq!(a, Action::Wait);
        assert!(matches!(nav.tick(t, Some(&online_menu())), Action::Select { .. }));
    }

    #[test]
    fn a_list_still_filling_in_resets_the_count() {
        let mut nav = Navigator::default();
        nav.then_matchmaking(false, Some(BLACKLIST));
        let (_, t) = until_done(&mut nav, Instant::now(), &svm_menu());
        // Blacklist shows with two entries, then the rest arrive: the count starts again each time.
        let partial = view(&[CLASSIC, BLACKLIST], 2, Some(0));
        let (a, t) = run(&mut nav, t, STABLE_TICKS - 1, Some(&partial));
        assert_eq!(a, Action::Wait);
        let (a, t) = run(&mut nav, t, STABLE_TICKS - 1, Some(&mode_list()));
        assert_eq!(a, Action::Wait);
        let (a, _) = run(&mut nav, t, 1, Some(&mode_list()));
        assert_eq!(
            a,
            Action::Select {
                goal: Goal::Mode(Some(BLACKLIST)),
                pos: 1,
                id: BLACKLIST,
                fallback: false
            }
        );
    }

    #[test]
    fn the_entry_going_away_resets_the_count() {
        let mut nav = Navigator::to_lobby(false);
        let (_, t) = run(&mut nav, Instant::now(), STABLE_TICKS - 1, Some(&online_menu()));
        let (_, t) = run(&mut nav, t, 1, Some(&view(&[7], 0, Some(0))));
        let (a, _) = run(&mut nav, t, STABLE_TICKS - 1, Some(&online_menu()));
        assert_eq!(a, Action::Wait);
    }

    #[test]
    fn a_matchmaking_step_not_done_in_a_minute_is_given_up_and_the_rest_dropped() {
        for stuck_at in 0..2 {
            let mut nav = Navigator::default();
            nav.then_matchmaking(false, Some(CLASSIC));
            let mut t = Instant::now();
            if stuck_at == 1 {
                t = until_done(&mut nav, t, &svm_menu()).1;
            }
            // Somewhere else (the player went to Private match's page, say): nothing to pick.
            let elsewhere = view(&[42, 43], 3, Some(0));
            let (a, t2) = run(&mut nav, t, 10, Some(&elsewhere));
            assert_eq!(a, Action::Wait);
            let late = t2 + STEP_GIVE_UP;
            let goal = if stuck_at == 0 { Goal::Matchmaking { coop: false } } else { Goal::Mode(Some(CLASSIC)) };
            assert_eq!(nav.tick(late, Some(&elsewhere)), Action::GiveUp(goal));
            assert!(!nav.busy());
            assert_eq!(nav.tick(late, Some(&mode_list())), Action::Idle, "nothing selected after giving up");
        }
    }

    #[test]
    fn each_step_gets_its_own_minute() {
        let mut nav = Navigator::default();
        nav.then_matchmaking(false, Some(CLASSIC));
        // Quick match is selected 50 s in; the mode list then has a minute of its own.
        let t = Instant::now();
        assert_eq!(nav.tick(t, None), Action::Wait);
        let (a, t) = until_done(&mut nav, t + Duration::from_secs(50), &svm_menu());
        assert!(matches!(a, Action::Select { id: ITEM_QUICK_MATCH, .. }));
        let (a, _) = until_done(&mut nav, t + Duration::from_secs(50), &mode_list());
        assert!(matches!(a, Action::Select { id: CLASSIC, .. }), "{a:?}");
    }

    #[test]
    fn the_lobby_step_has_no_minute_of_its_own() {
        let mut nav = Navigator::to_lobby(false);
        let t = Instant::now();
        assert_eq!(nav.tick(t, None), Action::Wait);
        assert_eq!(nav.tick(t + Duration::from_secs(240), None), Action::Wait);
    }

    #[test]
    fn esc_stops_every_step_at_any_point() {
        for at in 0..4 {
            let mut nav = Navigator::to_lobby(false);
            nav.then_matchmaking(false, Some(BLACKLIST));
            let mut t = Instant::now();
            for v in [online_menu(), svm_menu(), mode_list()].iter().take(at) {
                t = until_done(&mut nav, t, v).1;
            }
            assert_eq!(nav.abort(), at < 3, "abort at step {at}");
            assert!(!nav.busy());
            for v in [online_menu(), svm_menu(), mode_list()] {
                assert_eq!(run(&mut nav, t, STABLE_TICKS * 2, Some(&v)).0, Action::Idle);
            }
        }
    }

    #[test]
    fn half_way_counts_dont_carry_into_the_next_step() {
        let mut nav = Navigator::to_lobby(false);
        nav.then_matchmaking(false, None);
        let (_, t) = until_done(&mut nav, Instant::now(), &online_menu());
        // The next step starts from nothing, even with a list that has its entry at once.
        let (a, _) = run(&mut nav, t, STABLE_TICKS - 1, Some(&svm_menu()));
        assert_eq!(a, Action::Wait);
    }
}
