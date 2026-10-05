//! Maintenance booked on the player's server, or on the whole network (its coordinator), as
//! the directory lists it: a notice under the launch bar in the update notice's style
//! (update_notice.rs), in the player's own time, and a small tag in the server menu.
//!
//! A window shows from the earlier of the local midnight that begins its day and 12 hours
//! before it, until it ends; and after that while the server still doesn't answer.

use setup::clock::Clock;
use setup::directory::Listing;
use setup::directory::Window;

use crate::update_notice::city;
use crate::update_notice::place;
use crate::update_notice::Action;
use crate::update_notice::Kind;
use crate::update_notice::Notice;

/// From how long before its start a window is "soon": the notice offers another server.
const SOON: i64 = 30 * 60;
/// A window shows from at most this long before it, if its day begins later.
const AHEAD: i64 = 12 * 3600;

/// Where a window is, for the player now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Later today (or tonight, within 12 hours).
    Before,
    /// Within 30 minutes.
    Soon,
    /// Under way.
    During,
    /// Ended, and the server isn't back.
    Overrun,
}

/// When `window` starts showing: the local midnight that begins its day, or 12 hours
/// before it, whichever is earlier.
pub fn shows_from(window: &Window, clock: &Clock) -> i64 {
    clock.day_start(window.start).min(window.start - AHEAD)
}

/// Where `window` is now, if it shows. `answering`: the server answers now.
pub fn phase(window: &Window, now: i64, clock: &Clock, answering: bool) -> Option<Phase> {
    if now < shows_from(window, clock) {
        None
    } else if now < window.start {
        Some(if window.start - now <= SOON { Phase::Soon } else { Phase::Before })
    } else if now < window.end {
        Some(Phase::During)
    } else if !answering && now - window.end < setup::directory::ENDED_KEPT {
        Some(Phase::Overrun)
    } else {
        None
    }
}

/// The window to tell the player about, of a server's: one under way, else one it's late
/// back from (the latest), else the next to come.
pub fn showing<'a>(windows: &'a [Window], now: i64, clock: &Clock, answering: bool) -> Option<(&'a Window, Phase)> {
    let shown = || windows.iter().filter_map(move |w| phase(w, now, clock, answering).map(|p| (w, p)));
    shown()
        .find(|(_, p)| *p == Phase::During)
        .or_else(|| shown().filter(|(_, p)| *p == Phase::Overrun).max_by_key(|(w, _)| w.end))
        .or_else(|| shown().min_by_key(|(w, _)| w.start))
}

/// How long, in words: "about 15 minutes", "about 1 hour", "about 1½ hours".
pub fn length(secs: i64) -> String {
    if secs < 45 * 60 {
        return format!("about {} minutes", ((secs + 150) / 300 * 5).max(5));
    }
    let halves = (secs + 900) / 1800;
    match (halves / 2, halves % 2) {
        (1, 0) => "about 1 hour".into(),
        (h, 0) => format!("about {h} hours"),
        (h, _) => format!("about {h}½ hours"),
    }
}

/// A short time: "15 min", "~2 h".
fn minutes(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 90 * 60 {
        format!("{} min", ((secs + 30) / 60).max(1))
    } else {
        format!("~{} h", (secs + 1800) / 3600)
    }
}

/// "…for about 1 hour: Moving to a faster machine." or "…for about 1 hour."
fn with_note(text: String, note: &str) -> String {
    match note.trim() {
        "" => format!("{text}."),
        note if note.ends_with(['.', '!', '?']) => format!("{text}: {note}"),
        note => format!("{text}: {note}."),
    }
}

/// The server menu's tag on a server: "Maintenance 8 pm" when its window shows, "Back 9 pm"
/// while it's under way.
pub fn tag(server: &Listing, ping: Option<u32>, now: i64, clock: &Clock) -> Option<String> {
    let (window, phase) = showing(&server.maintenance, now, clock, ping.is_some())?;
    Some(match phase {
        Phase::Before | Phase::Soon => format!("Maintenance {}", clock.short_time(window.start)),
        Phase::During => format!("Back {}", clock.short_time(window.end)),
        Phase::Overrun => "Running late".into(),
    })
}

/// What the notices are worked out from.
pub struct Input<'a> {
    /// The network's servers, with this PC's ping to each (None: no answer).
    pub servers: &'a [(Listing, Option<u32>)],
    /// The player's server, with its ping now (None when it doesn't answer, or left the
    /// directory).
    pub mine: Option<(&'a Listing, Option<u32>)>,
    pub now: i64,
    pub clock: &'a Clock,
}

/// The server to offer instead: the lowest ping of those that answer and have no window of
/// their own before `until`.
fn alternative(input: &Input, until: i64) -> Option<(String, String, u32)> {
    let mine = input.mine.map(|(m, _)| m.host.as_str());
    input
        .servers
        .iter()
        .filter(|(s, ping)| ping.is_some() && Some(s.host.as_str()) != mine)
        .filter(|(s, _)| !s.maintenance.iter().any(|w| w.start < until && w.end > input.now))
        .min_by_key(|(_, ping)| *ping)
        .map(|(s, ping)| (s.host.clone(), place(s), ping.unwrap_or_default()))
}

fn play_on((host, alt, _): (String, String, u32)) -> (Action, String) {
    let label = format!("Play on {}", city(&alt));
    (Action::PlayOn { host, place: alt }, label)
}

/// The notice for maintenance on the player's server, if one shows.
pub fn notice(input: &Input) -> Option<Notice> {
    let (mine, ping) = input.mine?;
    let (now, clock) = (input.now, input.clock);
    let (window, phase) = showing(&mine.maintenance, now, clock, ping.is_some())?;
    let my_place = place(mine);
    let my_city = city(&my_place).to_string();
    let at = clock.clock_time(window.start);
    let back = clock.time(window.end, now);
    let long = length(window.end - window.start);
    let alt = alternative(input, window.end);
    let alt_line = |sentence: &str| {
        alt.as_ref()
            .map(|(_, place, ms)| format!(" {}", sentence.replace("{alt}", city(place)).replace("{ms}", &ms.to_string())))
            .unwrap_or_default()
    };
    let base = Notice {
        kind: Kind::Maintenance,
        heading: String::new(),
        eta: String::new(),
        text: String::new(),
        note: None,
        action: None,
        servers: vec![],
        status: format!("Ready to play · maintenance at {at}"),
        blocks_play: None,
    };
    Some(match phase {
        Phase::Before => Notice {
            heading: if clock.day_start(window.start) == clock.day_start(now) {
                "Maintenance today".into()
            } else {
                "Maintenance tomorrow".into()
            },
            eta: format!("{} · {}", clock.span(window.start, window.end), setup::clock::from_now(window.start - now)),
            text: format!(
                "{} Play until then, and finish your match before it starts: games on the server are cut off.",
                with_note(format!("{my_city} goes down at {at} for {long}"), &window.note)
            ),
            ..base
        },
        Phase::Soon => Notice {
            heading: "Maintenance soon".into(),
            eta: format!("{} · {}", setup::clock::from_now(window.start - now), clock.time(window.start, now)),
            text: format!(
                "{} A match you start now may not finish there.{}",
                with_note(format!("{my_city} goes down at {at} for {long}"), &window.note),
                alt_line("{alt} stays up ({ms} ms).")
            ),
            action: alt.map(play_on),
            ..base
        },
        Phase::During if ping.is_some() => Notice {
            kind: Kind::MaintenanceNow,
            heading: "Maintenance under way".into(),
            eta: format!("until about {back} · {} left", minutes(window.end - now)),
            text: format!(
                "{} It answers, so you can play, but it may restart and cut a match short.{}",
                with_note(format!("Maintenance on {my_city} is under way until about {back}"), &window.note),
                alt_line("{alt} stays up ({ms} ms).")
            ),
            action: alt.map(play_on),
            status: format!("Ready to play · maintenance until {}", clock.clock_time(window.end)),
            ..base
        },
        Phase::During => Notice {
            kind: Kind::MaintenanceNow,
            heading: "Your server is down for maintenance".into(),
            eta: format!("back about {back} · {} left", minutes(window.end - now)),
            text: format!(
                "{} The launcher checks every 15 seconds and says when it's back.{}",
                with_note(format!("{my_city} is down until about {back}"), &window.note),
                alt_line("Or play on {alt} now ({ms} ms).")
            ),
            action: alt.map(play_on),
            status: format!("{my_city} is down for maintenance"),
            blocks_play: Some(format!("{my_city} is down for maintenance until about {back}")),
            ..base
        },
        Phase::Overrun => Notice {
            kind: Kind::MaintenanceNow,
            heading: "Taking longer than planned".into(),
            eta: format!("due back {back} · {} over", minutes(now - window.end)),
            text: format!(
                "{my_city} should have been back at {back}. The launcher checks every 15 seconds and says when it is.{}",
                alt_line("Or play on {alt} now ({ms} ms).")
            ),
            action: alt.map(play_on),
            status: format!("{my_city} is down for maintenance"),
            blocks_play: Some(format!("{my_city} is down for maintenance, and taking longer than planned")),
            ..base
        },
    })
}

/// The notice for the network's own window, if it shows. It never stops anyone playing:
/// games carry on without the coordinator.
pub fn network_notice(window: Option<&Window>, now: i64, clock: &Clock) -> Option<Notice> {
    let window = window?;
    // Nothing says when the coordinator answers again: shown until its end.
    let phase = phase(window, now, clock, true)?;
    let at = clock.clock_time(window.start);
    let long = length(window.end - window.start);
    let back = clock.time(window.end, now);
    let base = Notice {
        kind: Kind::NetworkMaintenance,
        heading: "Network maintenance".into(),
        eta: format!("{} · {}", clock.span(window.start, window.end), setup::clock::from_now(window.start - now)),
        text: format!(
            "{} Your games carry on.",
            with_note(format!("Friends on other servers, the server list and new names pause from {at} for {long}"), &window.note)
        ),
        note: None,
        action: None,
        servers: vec![],
        status: format!("Ready to play · network maintenance at {at}"),
        blocks_play: None,
    };
    Some(match phase {
        Phase::Before if clock.day_start(window.start) != clock.day_start(now) => Notice {
            heading: "Network maintenance tomorrow".into(),
            ..base
        },
        Phase::Before => base,
        Phase::Soon => Notice {
            eta: format!("{} · {}", setup::clock::from_now(window.start - now), clock.time(window.start, now)),
            ..base
        },
        Phase::During | Phase::Overrun => Notice {
            heading: "Network maintenance under way".into(),
            eta: format!("back about {back} · {} left", minutes(window.end - now)),
            text: format!(
                "{} Your games carry on.",
                with_note(
                    format!("Friends on other servers, the server list and new names are paused until about {back}"),
                    &window.note
                )
            ),
            status: format!("Ready to play · network maintenance until {}", clock.clock_time(window.end)),
            ..base
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tue 6 Oct 2026, 07:00 UTC: 8:00 pm in New Zealand, 3:00 am in Toronto, 09:00 in
    /// Berlin. An hour long.
    const START: i64 = 1_791_270_000;
    const END: i64 = START + 3600;
    const NOTE: &str = "Moving to a faster machine";

    fn nz() -> Clock {
        Clock {
            offset: 13 * 3600,
            zone: "NZDT".into(),
            twelve_hour: true,
        }
    }

    fn toronto() -> Clock {
        Clock {
            offset: -4 * 3600,
            zone: "EDT".into(),
            twelve_hour: true,
        }
    }

    fn berlin() -> Clock {
        Clock {
            offset: 2 * 3600,
            zone: "CEST".into(),
            twelve_hour: false,
        }
    }

    fn window(start: i64, end: i64, note: &str) -> Window {
        Window { start, end, note: note.into() }
    }

    fn listing(id: &str, region: &str, windows: Vec<Window>) -> Listing {
        Listing {
            id: id.into(),
            name: format!("Example {id}"),
            region: region.into(),
            host: format!("{id}.example.net"),
            ports: None,
            version: "0.4.2".into(),
            players_online: 0,
            players_total: 0,
            friends_mode: String::new(),
            maintenance: windows,
        }
    }

    /// Sydney booked; Beauharnois and Falkenstein up.
    fn network() -> Vec<(Listing, Option<u32>)> {
        vec![
            (listing("oce", "Sydney, Australia", vec![window(START, END, NOTE)]), Some(45)),
            (listing("na", "Beauharnois, Canada", vec![]), Some(210)),
            (listing("eu", "Falkenstein, Germany", vec![]), Some(302)),
        ]
    }

    fn work(servers: &[(Listing, Option<u32>)], my_ping: Option<u32>, now: i64, clock: &Clock) -> Option<Notice> {
        let mine = servers.iter().find(|(s, _)| s.id == "oce").map(|(s, _)| (s, my_ping));
        notice(&Input { servers, mine, now, clock })
    }

    #[test]
    fn shows_on_the_day_or_from_12_hours_before() {
        let w = window(START, END, "");
        // New Zealand: from midnight that Tuesday, 20 hours before.
        assert_eq!(shows_from(&w, &nz()), START - 20 * 3600);
        assert_eq!(phase(&w, START - 24 * 3600, &nz(), true), None, "the day before");
        assert_eq!(phase(&w, START - 5 * 3600, &nz(), true), Some(Phase::Before));
        // Toronto: it starts at 3:00 am, so from 12 hours before (3:00 pm the day before).
        assert_eq!(shows_from(&w, &toronto()), START - 12 * 3600);
        assert_eq!(phase(&w, START - 30 * 60, &nz(), true), Some(Phase::Soon));
        assert_eq!(phase(&w, START + 60, &nz(), true), Some(Phase::During));
        assert_eq!(phase(&w, END + 600, &nz(), false), Some(Phase::Overrun));
        assert_eq!(phase(&w, END + 600, &nz(), true), None, "back");
    }

    #[test]
    fn earlier_that_day() {
        let n = work(&network(), Some(45), START - 5 * 3600, &nz()).unwrap();
        assert_eq!(n.kind, Kind::Maintenance);
        assert_eq!(n.heading, "Maintenance today");
        assert_eq!(n.eta, "8:00–9:00 pm NZDT · in ~5 h");
        assert_eq!(
            n.text,
            "Sydney goes down at 8:00 pm for about 1 hour: Moving to a faster machine. Play until then, and finish your match before it starts: games on the server are cut off."
        );
        assert_eq!(n.status, "Ready to play · maintenance at 8:00 pm");
        assert_eq!((n.action, n.blocks_play), (None, None));
    }

    #[test]
    fn early_in_the_morning_is_tomorrow() {
        let n = work(&network(), Some(30), START - 5 * 3600, &toronto()).unwrap();
        assert_eq!(n.heading, "Maintenance tomorrow");
        assert_eq!(n.eta, "3:00–4:00 am EDT · in ~5 h");
        let n = work(&network(), Some(30), START - 5 * 3600, &berlin()).unwrap();
        assert_eq!((n.heading.as_str(), n.eta.as_str()), ("Maintenance today", "09:00–10:00 CEST · in ~5 h"));
    }

    #[test]
    fn soon_offers_the_nearest_server_that_stays_up() {
        let mut servers = network();
        // Beauharnois has its own window then: Falkenstein, though farther.
        servers[1].0.maintenance = vec![window(START + 1800, START + 5400, "")];
        let n = work(&servers, Some(45), START - 15 * 60, &nz()).unwrap();
        assert_eq!(n.heading, "Maintenance soon");
        assert_eq!(n.eta, "in ~15 min · 8:00 pm NZDT");
        assert_eq!(
            n.text,
            "Sydney goes down at 8:00 pm for about 1 hour: Moving to a faster machine. A match you start now may not finish there. Falkenstein stays up (302 ms)."
        );
        assert_eq!(
            n.action,
            Some((
                Action::PlayOn {
                    host: "eu.example.net".into(),
                    place: "Falkenstein, Germany".into()
                },
                "Play on Falkenstein".into()
            ))
        );
        assert_eq!(n.blocks_play, None);
        let n = work(&network(), Some(45), START - 15 * 60, &nz()).unwrap();
        assert_eq!(n.action.map(|(_, label)| label).as_deref(), Some("Play on Beauharnois"));
    }

    #[test]
    fn under_way_blocks_play_while_the_server_is_down() {
        let n = work(&network(), None, START + 20 * 60, &nz()).unwrap();
        assert_eq!(n.kind, Kind::MaintenanceNow);
        assert_eq!(n.heading, "Your server is down for maintenance");
        assert_eq!(n.eta, "back about 9:00 pm NZDT · 40 min left");
        assert_eq!(
            n.text,
            "Sydney is down until about 9:00 pm NZDT: Moving to a faster machine. The launcher checks every 15 seconds and says when it's back. Or play on Beauharnois now (210 ms)."
        );
        assert!(n.blocks_play.is_some());
        assert_eq!(n.status, "Sydney is down for maintenance");
        assert_eq!(n.refresh_every(), std::time::Duration::from_secs(15));
        // It answers: Play stays, and the notice says it may restart.
        let n = work(&network(), Some(45), START + 20 * 60, &nz()).unwrap();
        assert_eq!(n.heading, "Maintenance under way");
        assert_eq!(n.eta, "until about 9:00 pm NZDT · 40 min left");
        assert!(n.text.contains("may restart"), "{}", n.text);
        assert_eq!(n.blocks_play, None);
    }

    #[test]
    fn overrunning_then_back() {
        let n = work(&network(), None, END + 10 * 60, &nz()).unwrap();
        assert_eq!(n.heading, "Taking longer than planned");
        assert_eq!(n.eta, "due back 9:00 pm NZDT · 10 min over");
        assert!(n.text.starts_with("Sydney should have been back at 9:00 pm NZDT."), "{}", n.text);
        assert!(n.blocks_play.is_some());
        assert_eq!(work(&network(), Some(45), END + 15 * 60, &nz()), None, "back: nothing to say");
        assert_eq!(work(&network(), Some(45), START - 24 * 3600, &nz()), None, "the day before");
    }

    #[test]
    fn without_a_note_or_another_server() {
        let servers = vec![(listing("oce", "Sydney, Australia", vec![window(START, START + 15 * 60, "")]), Some(45))];
        let n = work(&servers, Some(45), START - 10 * 60, &nz()).unwrap();
        assert_eq!(n.text, "Sydney goes down at 8:00 pm for about 15 minutes. A match you start now may not finish there.");
        assert_eq!(n.action, None);
    }

    #[test]
    fn lengths_in_words() {
        assert_eq!(length(15 * 60), "about 15 minutes");
        assert_eq!(length(60), "about 5 minutes");
        assert_eq!(length(40 * 60), "about 40 minutes");
        assert_eq!(length(45 * 60), "about 1 hour");
        assert_eq!(length(3600), "about 1 hour");
        assert_eq!(length(5400), "about 1½ hours");
        assert_eq!(length(7200), "about 2 hours");
        assert_eq!(length(24 * 3600), "about 24 hours");
    }

    #[test]
    fn server_menu_tags() {
        let (oce, _) = &network()[0];
        assert_eq!(tag(oce, Some(45), START - 5 * 3600, &nz()).as_deref(), Some("Maintenance 8 pm"));
        assert_eq!(tag(oce, None, START + 60, &nz()).as_deref(), Some("Back 9 pm"));
        assert_eq!(tag(oce, Some(45), START - 5 * 3600, &berlin()).as_deref(), Some("Maintenance 09:00"));
        assert_eq!(tag(oce, Some(45), START - 24 * 3600, &nz()), None, "not showing yet");
        assert_eq!(tag(oce, None, END + 600, &nz()).as_deref(), Some("Running late"));
        assert_eq!(tag(oce, Some(45), END + 600, &nz()), None);
    }

    #[test]
    fn the_network_itself() {
        let w = window(START, END, "");
        let n = network_notice(Some(&w), START - 5 * 3600, &nz()).unwrap();
        assert_eq!(n.kind, Kind::NetworkMaintenance);
        assert_eq!(n.heading, "Network maintenance");
        assert_eq!(n.eta, "8:00–9:00 pm NZDT · in ~5 h");
        assert_eq!(
            n.text,
            "Friends on other servers, the server list and new names pause from 8:00 pm for about 1 hour. Your games carry on."
        );
        assert_eq!(n.blocks_play, None, "never blocks Play");
        let n = network_notice(Some(&w), START + 20 * 60, &nz()).unwrap();
        assert_eq!(n.heading, "Network maintenance under way");
        assert_eq!(n.blocks_play, None);
        assert_eq!(network_notice(Some(&w), END + 60, &nz()), None);
        assert_eq!(network_notice(None, START, &nz()), None);
    }

    #[test]
    fn the_most_pressing_notice_wins() {
        let clock = nz();
        let mine = work(&network(), None, START + 60, &clock);
        let soon = work(&network(), Some(45), START - 600, &clock);
        let net = network_notice(Some(&window(START, END, "")), START - 600, &clock);
        let winner = crate::update_notice::most_pressing([net.clone(), mine]).unwrap();
        assert_eq!(winner.kind, Kind::MaintenanceNow);
        assert_eq!(crate::update_notice::most_pressing([net, soon]).unwrap().kind, Kind::Maintenance);
        assert_eq!(crate::update_notice::most_pressing([None, None]), None);
    }
}
