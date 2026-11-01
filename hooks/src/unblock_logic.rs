//! Pure decision logic of `follow_unblock`: when is the member's UI facade stuck behind a follow
//! task that nobody will answer? No Windows types in here, so the unit tests also run on a Linux
//! host (`rustc --test unblock_logic.rs`).

/// Facade state 7: "follow the party leader" (`txt_Migration` case 7).
pub const STATE_FOLLOW: u32 = 7;
/// Facade phase 2: the task was submitted and is running.
pub const PHASE_RUNNING: u32 = 2;
/// `[err+4]` of the pending error: 1 = the host was lost.
pub const ERROR_HOST_LOST: u32 = 1;
/// The condition must hold on this many consecutive ticks (~50 ms apart) before the facade is touched.
pub const REQUIRED_TICKS: u32 = 2;

/// Whether the stuck-state condition holds on this tick (without the tick count). Every input is
/// `None` when it could not be read, which never counts as a match.
///
/// * `state34`/`phase38`: the facade's state machine, stuck at 7/2.
/// * `party414`: the party controller's leader-action state, 0 once the goal consumed it.
/// * `error_cat`: category (`[err+4]`) of the pending `NetOnline` error, `None` if there is none.
/// * `gate_vtable`: first dword of the task in the facade's task slot, `None` if the slot is empty.
/// * `task_vtable`: the vtable of `PartyFollowLeaderTask` seen at its `vf04` (0 = not seen yet).
pub fn conditions_hold(state34: Option<u32>, phase38: Option<u32>, party414: Option<u32>, error_cat: Option<u32>, gate_vtable: Option<u32>, task_vtable: u32) -> bool {
    state34 == Some(STATE_FOLLOW)
        && phase38 == Some(PHASE_RUNNING)
        && party414 == Some(0)
        && error_cat == Some(ERROR_HOST_LOST)
        && task_vtable != 0
        && gate_vtable == Some(task_vtable)
}

/// The decision: the condition holds now and `ticks` (consecutive ticks it has held, this one
/// included) has reached [`REQUIRED_TICKS`].
pub fn should_unblock(state34: Option<u32>, phase38: Option<u32>, party414: Option<u32>, error_cat: Option<u32>, gate_vtable: Option<u32>, task_vtable: u32, ticks: u32) -> bool {
    ticks >= REQUIRED_TICKS && conditions_hold(state34, phase38, party414, error_cat, gate_vtable, task_vtable)
}

/// The next consecutive-tick count: one more while the condition holds, else back to 0.
pub fn next_ticks(held: bool, ticks: u32) -> u32 {
    if held {
        ticks.saturating_add(1)
    } else {
        0
    }
}

/// A follow that ended this recently (ms) still counts as "the first follow".
pub const SECOND_FOLLOW_WINDOW_MS: u64 = 5000;
/// Highest value of the party controller's leader-action state `+0x414` (1..=6 = a follow is pending).
pub const LEADER_ACTION_MAX: u32 = 6;

/// H8: drop the duplicate follow. True when the facade is idle (`state34 == 0`, `phase38 == 0`,
/// task slot `gate` readable and empty), a follow is pending (`party414` in 1..=6) and a
/// `FollowDelegate` with result 0 finished less than [`SECOND_FOLLOW_WINDOW_MS`] ago.
/// `None` = unreadable / never happened, which never matches.
pub fn should_drop_second_follow(state34: Option<u32>, phase38: Option<u32>, gate: Option<u32>, party414: Option<u32>, ms_since_done: Option<u64>) -> bool {
    state34 == Some(0)
        && phase38 == Some(0)
        && gate == Some(0)
        && matches!(party414, Some(1..=LEADER_ACTION_MAX))
        && matches!(ms_since_done, Some(ms) if ms < SECOND_FOLLOW_WINDOW_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VT: u32 = 0x0140_1234;

    fn stuck(ticks: u32) -> bool {
        should_unblock(Some(7), Some(2), Some(0), Some(1), Some(VT), VT, ticks)
    }

    #[test]
    fn needs_two_consecutive_ticks() {
        assert!(!stuck(0));
        assert!(!stuck(1));
        assert!(stuck(2));
        assert!(stuck(3));
    }

    #[test]
    fn any_deviation_blocks() {
        let t = REQUIRED_TICKS;
        assert!(!should_unblock(Some(0), Some(2), Some(0), Some(1), Some(VT), VT, t), "state 0 is healthy");
        assert!(!should_unblock(Some(7), Some(1), Some(0), Some(1), Some(VT), VT, t), "phase 1 is still starting");
        assert!(!should_unblock(Some(7), Some(2), Some(6), Some(1), Some(VT), VT, t), "a follow is still pending");
        assert!(!should_unblock(Some(7), Some(2), Some(0), None, Some(VT), VT, t), "no error yet");
        assert!(!should_unblock(Some(7), Some(2), Some(0), Some(0), Some(VT), VT, t), "other error category");
        assert!(!should_unblock(Some(7), Some(2), Some(0), Some(2), Some(VT), VT, t), "other error category");
        assert!(!should_unblock(Some(7), Some(2), Some(0), Some(1), None, VT, t), "slot empty");
        assert!(!should_unblock(Some(7), Some(2), Some(0), Some(1), Some(VT + 4), VT, t), "another kind of task");
    }

    #[test]
    fn unknown_task_vtable_never_matches() {
        assert!(!should_unblock(Some(7), Some(2), Some(0), Some(1), Some(0), 0, 5));
        assert!(!should_unblock(Some(7), Some(2), Some(0), Some(1), Some(VT), 0, 5));
    }

    #[test]
    fn unreadable_values_never_match() {
        assert!(!should_unblock(None, Some(2), Some(0), Some(1), Some(VT), VT, 5));
        assert!(!should_unblock(Some(7), None, Some(0), Some(1), Some(VT), VT, 5));
        assert!(!should_unblock(Some(7), Some(2), None, Some(1), Some(VT), VT, 5));
    }

    #[test]
    fn tick_counter_resets_when_the_condition_breaks() {
        let mut t = 0;
        t = next_ticks(true, t);
        assert_eq!(t, 1);
        t = next_ticks(true, t);
        assert_eq!(t, 2);
        t = next_ticks(false, t);
        assert_eq!(t, 0);
        assert_eq!(next_ticks(true, u32::MAX), u32::MAX);
    }

    #[test]
    fn second_follow_is_dropped_only_in_the_measured_situation() {
        let d = |s, p, g, c, m| should_drop_second_follow(s, p, g, c, m);
        assert!(d(Some(0), Some(0), Some(0), Some(6), Some(10)));
        assert!(d(Some(0), Some(0), Some(0), Some(1), Some(4999)));
        assert!(!d(Some(0), Some(0), Some(0), Some(6), Some(5000)), "window over");
        assert!(!d(Some(0), Some(0), Some(0), Some(6), None), "no first follow seen");
        assert!(!d(Some(0), Some(0), Some(0), Some(0), Some(10)), "nothing pending");
        assert!(!d(Some(0), Some(0), Some(0), Some(7), Some(10)), "out of range");
        assert!(!d(Some(7), Some(0), Some(0), Some(6), Some(10)), "facade busy");
        assert!(!d(Some(0), Some(2), Some(0), Some(6), Some(10)), "facade busy");
        assert!(!d(Some(0), Some(0), Some(0x1234), Some(6), Some(10)), "task in slot");
        assert!(!d(Some(0), Some(0), None, Some(6), Some(10)), "slot unreadable");
        assert!(!d(None, Some(0), Some(0), Some(6), Some(10)));
    }
}
