//! Watched objects in memory (0.5.6's D1, report only). F8 hashes the game's code; a cheat table
//! edits data too: the match manager's rules and switches. This reads every integer, float, byte
//! and bool property the live match manager's class (and the classes it's made from) declares,
//! keeps them as a round starts, and every [`EVERY`] while it runs says which changed, by name,
//! before and after. The game changes some by itself as a round goes (timers, the round state,
//! scores, the hack's progress): those are [`EXPECTED`] and left out. Others are said to the server
//! once a match (`fair_play`, check `watched`).
//!
//! The expected list comes from clean matches on the gamebox (every change logged with the
//! test build's `watched` command). Until a name is known to change by itself it isn't sent:
//! [`REPORTING`] stays off until that list is filled from the gamebox, so no player is noted
//! for the game's own changes (the lesson of FN).

use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

use tracing::info;

use crate::dataversion::read;
use crate::dataversion::read_u32;
use crate::round_state as rs;

/// How often a round's properties are compared with its start.
const EVERY: Duration = Duration::from_secs(30);
/// Whether changes are said to the server (else only logged): on once [`EXPECTED`] is filled.
const REPORTING: bool = false;
/// The most properties watched.
const MOST: usize = 2048;

/// Names the game changes by itself during a round (the gamebox's clean matches), and parts of
/// names that always do (timers, states, counts).
const EXPECTED: &[&str] = &[];
const EXPECTED_PARTS: &[&str] = &["Timer", "Time", "State", "Round", "Score", "Num", "Count", "Progress", "Hack", "Current", "Last", "Delay", "Kill", "Winning"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Int,
    Float,
    Byte,
    Bool(u32),
}

#[derive(Clone, Debug)]
struct Prop {
    name: String,
    offset: u16,
    kind: Kind,
}

struct Watched {
    object: usize,
    class: u32,
    props: Vec<Prop>,
    base: Option<Vec<u32>>,
    read_at: Option<Instant>,
    said: bool,
}

static WATCHED: Mutex<Option<Watched>> = Mutex::new(None);

/// The properties `class` (and the classes it's made from) declares, by the engine's tables.
fn props_of(class: usize) -> Vec<Prop> {
    let names = rs::name_indices(&["IntProperty", "FloatProperty", "ByteProperty", "BoolProperty"]);
    let kinds: Vec<(u32, &str)> = ["IntProperty", "FloatProperty", "ByteProperty", "BoolProperty"]
        .iter()
        .filter_map(|k| Some((rs::pack(*names.get(k)?, 0), *k)))
        .collect();
    let chain = rs::chain(class);
    let mut out = Vec::new();
    rs::each_object(|o| {
        if out.len() >= MOST || !read_u32(o + 0x18).is_some_and(|outer| chain.contains(&outer)) {
            return;
        }
        let Some((_, kind)) = kinds.iter().find(|(packed, _)| rs::of_class(o, *packed)) else { return };
        let Some(offset) = read::<u16>(o + 0x3a) else { return };
        let kind = match *kind {
            "IntProperty" => Kind::Int,
            "FloatProperty" => Kind::Float,
            "ByteProperty" => Kind::Byte,
            _ => match read_u32(o + rs::BOOL_MASK_AT).filter(|m| m.count_ones() == 1) {
                Some(mask) => Kind::Bool(mask),
                None => return,
            },
        };
        let name = read_u32(o + 0x20).and_then(rs::name_text).unwrap_or_else(|| format!("+{offset}"));
        out.push(Prop { name, offset, kind });
    });
    out.sort_by_key(|p| p.offset);
    out
}

fn value(object: usize, p: &Prop) -> Option<u32> {
    let at = object + usize::from(p.offset);
    match p.kind {
        Kind::Int | Kind::Float => read_u32(at),
        Kind::Byte => read::<u8>(at).map(u32::from),
        Kind::Bool(mask) => read_u32(at).map(|w| u32::from(w & mask != 0)),
    }
}

fn shown(p: &Prop, v: u32) -> String {
    match p.kind {
        Kind::Float => format!("{}", f32::from_bits(v)),
        Kind::Int => format!("{}", v as i32),
        _ => v.to_string(),
    }
}

/// Whether the game changes `name` by itself.
fn expected(name: &str) -> bool {
    EXPECTED.contains(&name) || EXPECTED_PARTS.iter().any(|part| name.contains(part))
}

/// The changes between `base` and `now`, not expected: "name before→after".
fn changes(props: &[Prop], base: &[u32], now: &[u32]) -> Vec<String> {
    props
        .iter()
        .zip(base.iter().zip(now))
        .filter(|(p, (b, n))| b != n && !expected(&p.name))
        .map(|(p, (b, n))| format!("{} {}→{}", p.name, shown(p, *b), shown(p, *n)))
        .collect()
}

/// From the match state's loop: the live match manager `object`, whether its round is under way.
pub fn tick(object: Option<usize>, under_way: bool) {
    let mut guard = WATCHED.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(object) = object else {
        *guard = None;
        return;
    };
    let Some(class) = read_u32(object + 0x24) else { return };
    if guard.as_ref().is_none_or(|w| w.object != object || w.class != class) {
        let props = props_of(class as usize);
        info!("Watched: the match manager's {} properties", props.len());
        *guard = Some(Watched { object, class, props, base: None, read_at: None, said: false });
    }
    let Some(w) = guard.as_mut() else { return };
    if !under_way {
        w.base = None;
        return;
    }
    let read_all = |w: &Watched| -> Vec<u32> { w.props.iter().map(|p| value(object, p).unwrap_or(0)).collect() };
    let Some(base) = &w.base else {
        w.base = Some(read_all(w));
        w.read_at = Some(Instant::now());
        return;
    };
    if w.read_at.is_some_and(|at| at.elapsed() < EVERY) {
        return;
    }
    w.read_at = Some(Instant::now());
    let now = read_all(w);
    let found = changes(&w.props, base, &now);
    if found.is_empty() {
        return;
    }
    info!("Watched: the match manager changed during the round: {}", found.join(", "));
    if REPORTING && !w.said {
        w.said = true;
        let detail = format!("the match manager changed during the round: {}", found.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
        let n = found.len();
        let _ = std::thread::Builder::new().name("fe-watched".into()).spawn(move || {
            #[allow(clippy::cast_precision_loss)]
            crate::api::fair_play_check("watched", &detail, n as f64, hooks_config::running_under_wine());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unexpected_changes_are_said() {
        let p = |name: &str, kind| Prop { name: name.into(), offset: 0, kind };
        let props = [p("m_fRoundStateTimer", Kind::Float), p("m_bIsFriendlyFireEnabled", Kind::Bool(1)), p("m_iScoreLimit", Kind::Int), p("m_iSomething", Kind::Int)];
        let base = [1.0f32.to_bits(), 0, 3, 5];
        let now = [9.0f32.to_bits(), 1, 3, (-1i32) as u32];
        assert_eq!(changes(&props, &base, &now), vec!["m_bIsFriendlyFireEnabled 0→1", "m_iSomething 5→-1"]);
    }
}
