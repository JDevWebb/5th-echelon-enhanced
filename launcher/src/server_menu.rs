//! The server menu on the Play screen: the network's servers, best first, picked in one
//! click. Anything else (a server of your own, a LAN party's, another network) is on the
//! Servers screen.

use eframe::egui;
use eframe::egui::RichText;
use setup::directory::Listing;

use crate::theme;

/// One server in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub host: String,
    /// Where it is ("Sydney, Australia"), or its name when it doesn't say.
    pub place: String,
    /// The network's name for it, without the words every server's name starts with
    /// ("Oceania" for "5th Echelon Community Oceania").
    pub name: String,
    pub ping: Option<u32>,
    pub players: u32,
    /// The player is on it.
    pub current: bool,
    pub best: bool,
}

/// The servers in the order to offer them: the best first ([`setup::directory::best`]),
/// then by ping (no answer last), then the busiest.
pub fn ranked(servers: &[(Listing, Option<u32>)]) -> Vec<usize> {
    let best = setup::directory::best(servers);
    let mut order: Vec<usize> = (0..servers.len()).collect();
    order.sort_by_key(|&i| {
        let (listing, ping) = &servers[i];
        (Some(i) != best, ping.is_none(), ping.unwrap_or(u32::MAX), std::cmp::Reverse(listing.players_online), i)
    });
    order
}

/// The menu's rows, best first, for the player on `current` (a host, as their settings
/// have it).
pub fn rows(servers: &[(Listing, Option<u32>)], current: &str) -> Vec<Row> {
    let best = setup::directory::best(servers);
    let names = short_names(&servers.iter().map(|(s, _)| s.name.as_str()).collect::<Vec<_>>());
    ranked(servers)
        .into_iter()
        .map(|i| {
            let (s, ping) = &servers[i];
            Row {
                host: s.host.clone(),
                place: if s.region.is_empty() { s.name.clone() } else { s.region.clone() },
                name: names[i].clone(),
                ping: *ping,
                players: s.players_online,
                current: s.host.eq_ignore_ascii_case(current.trim()),
                best: best == Some(i),
            }
        })
        .collect()
}

/// The names without the words they all start with, when there are several and something
/// is left of each.
fn short_names(names: &[&str]) -> Vec<String> {
    let words: Vec<Vec<&str>> = names.iter().map(|n| n.split_whitespace().collect()).collect();
    let shortest = words.iter().map(Vec::len).min().unwrap_or(0);
    let common = (0..shortest).take_while(|&k| words.iter().all(|w| w[k] == words[0][k])).count();
    if names.len() < 2 || common == 0 || words.iter().any(|w| w.len() == common) {
        return names.iter().map(|n| (*n).to_string()).collect();
    }
    words.iter().map(|w| w[common..].join(" ")).collect()
}

/// A ping, coloured by how it plays: green is good, amber is far.
pub fn ping_text(ping: Option<u32>) -> RichText {
    match ping {
        None => theme::muted("no answer"),
        Some(ms) => RichText::new(format!("{ms} ms")).monospace().color(ping_color(ms)),
    }
}

pub fn ping_color(ms: u32) -> egui::Color32 {
    match ms {
        0..=90 => theme::OK,
        91..=180 => theme::FG,
        _ => theme::WARN,
    }
}

/// What the player did in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Server(String),
    PingAgain,
    ServersScreen,
}

/// The menu's contents: its header, a row a server, and the way to the Servers screen.
/// `focus` is the row the arrow keys are on.
pub fn show(ui: &mut egui::Ui, header: &str, rows: &[Row], pinging: bool, note: Option<&str>, focus: &mut usize) -> Option<Picked> {
    let mut picked = None;
    // Arrows move, Enter picks; Esc closes the popup itself.
    if !rows.is_empty() {
        let (down, up, enter) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        *focus = (*focus).min(rows.len() - 1);
        if down {
            *focus = (*focus + 1) % rows.len();
        }
        if up {
            *focus = (*focus + rows.len() - 1) % rows.len();
        }
        if enter && !rows[*focus].current {
            picked = Some(Picked::Server(rows[*focus].host.clone()));
        }
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(theme::caps(header));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            if pinging {
                ui.spinner();
            } else if ui.link(RichText::new("Ping again").color(theme::ACCENT)).clicked() {
                picked = Some(Picked::PingAgain);
            }
        });
    });
    ui.add_space(4.0);
    if rows.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(theme::muted(if pinging {
                "Pinging the servers…"
            } else {
                "This network lists no servers right now."
            }));
        });
    }
    for (i, row) in rows.iter().enumerate() {
        let response = row_ui(ui, row, i == *focus);
        if response.hovered() {
            *focus = i;
        }
        if response.clicked() && !row.current {
            picked = Some(Picked::Server(row.host.clone()));
        }
    }
    if let Some(note) = note {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.label(theme::muted(note).small());
        });
    }
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 9.0), egui::Sense::hover());
    ui.painter().hline(line.x_range().shrink(4.0), line.center().y, egui::Stroke::new(1.0, theme::LINE));
    let last = ui
        .horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(RichText::new("A server of your own, a LAN server or another network").color(theme::SOFT).size(13.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("Servers ›").family(theme::strong()).color(theme::ACCENT).size(13.0));
            });
        })
        .response;
    if ui
        .interact(last.rect.expand2(egui::vec2(0.0, 4.0)), ui.id().with("to-servers"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        picked = Some(Picked::ServersScreen);
    }
    ui.add_space(4.0);
    picked
}

/// One server: a tick on the current one, where it is with a badge on the best, the
/// network's name for it and its host, the ping, and players online.
fn row_ui(ui: &mut egui::Ui, row: &Row, focused: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 50.0), egui::Sense::click());
    let response = response.on_hover_cursor(if row.current { egui::CursorIcon::Default } else { egui::CursorIcon::PointingHand });
    let p = ui.painter();
    if focused || response.hovered() {
        p.rect_filled(rect, 9, theme::CONTROL);
    }
    let left = rect.left() + 10.0;
    if row.current {
        let c = egui::pos2(left + 8.0, rect.center().y);
        let s = egui::Stroke::new(2.2, theme::ACCENT);
        p.line_segment([c + egui::vec2(-5.0, 0.0), c + egui::vec2(-1.5, 3.5)], s);
        p.line_segment([c + egui::vec2(-1.5, 3.5), c + egui::vec2(5.5, -4.0)], s);
    }
    let x = left + 28.0;
    let right_col = 150.0;
    let text_width = (rect.right() - right_col - x).max(60.0);
    let galley = |text: String, font: egui::FontId, color: egui::Color32| {
        let mut job = egui::text::LayoutJob::single_section(text, egui::TextFormat::simple(font, color));
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let title = galley(row.place.clone(), egui::FontId::new(15.0, theme::strong()), theme::FG);
    let title_width = title.size().x;
    let sub = galley(format!("{} · {}", row.name, row.host), egui::FontId::proportional(12.5), theme::MUTED);
    let top = rect.center().y - title.size().y;
    p.galley(egui::pos2(x, top), title, theme::FG);
    p.galley(egui::pos2(x, rect.center().y + 1.0), sub, theme::MUTED);
    let tag = if row.best {
        Some(("BEST FOR YOU", theme::ON_ACCENT, Some(theme::ACCENT)))
    } else if row.current {
        Some(("CURRENT", theme::OK, None))
    } else {
        None
    };
    if let Some((text, color, fill)) = tag {
        let tag = ui.fonts_mut(|f| f.layout_no_wrap(text.into(), egui::FontId::proportional(10.0), color));
        let at = egui::pos2(x + title_width + 8.0, top + 3.0);
        let tag_rect = egui::Rect::from_min_size(at, tag.size() + egui::vec2(12.0, 4.0));
        if tag_rect.right() < rect.right() - right_col {
            if let Some(fill) = fill {
                p.rect_filled(tag_rect, 4, fill);
            }
            p.galley(at + egui::vec2(6.0, 2.0), tag, color);
        }
    }
    let ping = match row.ping {
        Some(ms) => (format!("{ms} ms"), ping_color(ms)),
        None => ("no answer".to_string(), theme::MUTED),
    };
    p.text(
        egui::pos2(rect.right() - 82.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        ping.0,
        egui::FontId::monospace(13.5),
        ping.1,
    );
    p.text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{} online", row.players),
        egui::FontId::proportional(12.5),
        theme::MUTED,
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, row.current, &row.place));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(name: &str, region: &str, host: &str, players: u32) -> Listing {
        Listing {
            id: host.into(),
            name: name.into(),
            region: region.into(),
            host: host.into(),
            ports: None,
            version: String::new(),
            players_online: players,
            players_total: 0,
            friends_mode: String::new(),
        }
    }

    fn network() -> Vec<(Listing, Option<u32>)> {
        vec![
            (listing("Example Net Europe", "Falkenstein, Germany", "eu.example.net", 0), Some(366)),
            (listing("Example Net North America", "Beauharnois, Canada", "na.example.net", 2), Some(213)),
            (listing("Example Net Oceania", "Sydney, Australia", "oce.example.net", 0), Some(45)),
            (listing("Example Net Asia", "", "asia.example.net", 9), None),
        ]
    }

    #[test]
    fn best_first_then_by_ping() {
        let rows = rows(&network(), "na.example.net");
        let hosts: Vec<&str> = rows.iter().map(|r| r.host.as_str()).collect();
        assert_eq!(hosts, ["oce.example.net", "na.example.net", "eu.example.net", "asia.example.net"]);
        assert!(rows[0].best && !rows[1].best);
        assert!(rows[1].current && !rows[0].current);
        assert_eq!(ranked(&network()), [2, 1, 0, 3]);
    }

    #[test]
    fn labels_say_where_and_which() {
        let rows = rows(&network(), "OCE.example.net ");
        assert_eq!(rows[0].place, "Sydney, Australia");
        assert_eq!(rows[0].name, "Oceania");
        assert_eq!(rows[1].name, "North America");
        assert!(rows[0].current, "hosts compare without case or spaces");
        // No region: its name stands in.
        assert_eq!(rows[3].place, "Example Net Asia");
        assert_eq!((rows[3].ping, rows[3].players), (None, 9));
    }

    #[test]
    fn busier_server_wins_a_close_ping() {
        let servers = vec![(listing("A", "", "a.example.net", 0), Some(40)), (listing("B", "", "b.example.net", 5), Some(50))];
        let rows = rows(&servers, "");
        assert_eq!(rows[0].host, "b.example.net");
        assert!(rows[0].best);
    }

    #[test]
    fn names_lose_only_the_words_they_share() {
        assert_eq!(short_names(&["5th Echelon EU", "5th Echelon NA"]), ["EU", "NA"]);
        assert_eq!(short_names(&["Alpha", "Beta"]), ["Alpha", "Beta"]);
        // Nothing would be left of one: all kept as they are.
        assert_eq!(short_names(&["Net", "Net Two"]), ["Net", "Net Two"]);
        assert_eq!(short_names(&["Only One Server"]), ["Only One Server"]);
        assert!(short_names(&[]).is_empty());
    }
}
