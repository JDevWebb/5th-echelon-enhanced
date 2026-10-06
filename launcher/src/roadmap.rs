//! The Roadmap screen: what's being built, from the community coordinator's roadmap
//! (whatever network the player is on), kept on disk so it shows offline; a form to suggest
//! a feature, signed with the player's identity; and the player's own suggestions with the
//! admins' replies.

use std::time::Duration;
use std::time::Instant;

use eframe::egui;
use eframe::egui::RichText;
use setup::roadmap::Mine;

use crate::activity::Action;
use crate::app::App;
use crate::app::Notices;
use crate::task::Slot;
use crate::theme;

/// Where the roadmap and suggestions live: the project's own coordinator.
const COORDINATOR: &str = setup::directory::COMMUNITY;
/// How long the roadmap is kept on screen before it's read again.
const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
/// How long the project's server may take to answer.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A suggestion as sent, to send again.
#[derive(Debug, Clone)]
struct Draft {
    area: String,
    title: String,
    text: String,
    name: String,
    server: Option<String>,
}

#[derive(Default)]
pub struct Roadmap {
    loading: Slot<Result<setup::roadmap::Roadmap, String>>,
    /// The roadmap shown, when it was read (Unix seconds), and whether that was just now
    /// (not the copy kept from before).
    shown: Option<(setup::roadmap::Roadmap, i64, bool)>,
    cache_read: bool,
    loaded_at: Option<Instant>,
    error: Option<String>,

    /// The form: the area chosen (an index into `AREAS`), the title and the details.
    area: usize,
    title: String,
    text: String,
    sending: Slot<Result<(), String>>,
    sent: Option<(crate::activity::Handle, Draft)>,
    failed: Option<(u64, Draft)>,

    /// The player's own suggestions, once asked.
    fetching_mine: Slot<Result<Vec<Mine>, String>>,
    mine: Option<Vec<Mine>>,
    mine_error: Option<String>,
    mine_asked: bool,
    mine_at: Option<Instant>,
}

/// The project's server: a client that waits 10 s at most and follows no redirects.
pub(crate) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

/// The answer's body, as text, up to [`setup::roadmap::MAX_BYTES`]; or the server's own words
/// when it said no.
pub(crate) async fn body(resp: reqwest::Response) -> Result<String, String> {
    let status = resp.status();
    let mut resp = resp;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("The project's server stopped answering: {e}"))? {
        if body.len() + chunk.len() > setup::roadmap::MAX_BYTES {
            return Err("The project's server sent too much.".into());
        }
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    if !status.is_success() {
        return Err(setup::roadmap::error_text(status.as_u16(), &body));
    }
    Ok(body)
}

pub(crate) fn unreachable(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "The project's server didn't answer in time.".into()
    } else {
        "The project's server didn't answer. Check your connection and try again.".into()
    }
}

/// Reads the roadmap, and keeps it for when the server can't be reached.
fn fetch_roadmap() -> Result<setup::roadmap::Roadmap, String> {
    let roadmap = crate::services::rt().block_on(async {
        let resp = client()?
            .get(setup::roadmap::url(COORDINATOR))
            .header("accept", "application/json")
            .send()
            .await
            .map_err(unreachable)?;
        let body = body(resp).await?;
        setup::roadmap::parse(&body).map_err(|_| "The project's server sent a roadmap the launcher can't read.".to_string())
    })?;
    if let Some(dir) = setup::app_data_dir() {
        setup::roadmap::save_cache(&dir, &roadmap, identity::now());
    }
    Ok(roadmap)
}

/// The player's identity, which signs what they send.
pub(crate) fn load_identity() -> Result<identity::Identity, String> {
    match setup::player_identity::load() {
        Ok(Some(identity)) => Ok(identity),
        Ok(None) => Err("Connect to a server first: your identity, which signs suggestions, is made then.".into()),
        Err(e) => Err(format!("Your identity can't be read here: {e}")),
    }
}

/// Whether this PC has an identity yet (made when the player first connects).
pub(crate) fn has_identity() -> bool {
    setup::player_identity::path().is_some_and(|p| p.exists())
}

fn send(draft: &Draft) -> Result<(), String> {
    let identity = load_identity()?;
    let sender = setup::roadmap::Sender {
        name: &draft.name,
        server: draft.server.as_deref(),
        launcher: env!("FE_RELEASE"),
    };
    let json = setup::roadmap::suggestion_body(&identity, &sender, &draft.area, &draft.title, &draft.text, identity::now());
    crate::services::rt().block_on(async {
        let resp = client()?
            .post(format!("{COORDINATOR}/v1/suggestions"))
            .header("content-type", "application/json")
            .body(json.to_string())
            .send()
            .await
            .map_err(unreachable)?;
        body(resp).await.map(|_| ())
    })
}

fn fetch_mine() -> Result<Vec<Mine>, String> {
    let identity = load_identity()?;
    let url = setup::roadmap::mine_url(COORDINATOR, &identity, identity::now());
    crate::services::rt().block_on(async {
        let resp = client()?.get(url).header("accept", "application/json").send().await.map_err(unreachable)?;
        let body = body(resp).await?;
        setup::roadmap::parse_mine(&body).map_err(|_| "The project's server sent an answer the launcher can't read.".to_string())
    })
}

impl Roadmap {
    /// Every frame, on whatever screen: what came of sending a suggestion, and the activity
    /// bar's Try again.
    pub fn tick(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        if let Some(result) = self.sending.poll() {
            if let Some((activity, draft)) = self.sent.take() {
                match result {
                    Ok(()) => {
                        activity.done("Thanks! Your suggestion is with the project's admins.");
                        // The form was kept until it went; a new one starts empty.
                        if self.title.trim() == draft.title && self.text.replace("\r\n", "\n").trim() == draft.text {
                            self.title.clear();
                            self.text.clear();
                        }
                        self.ask_mine(ctx);
                    }
                    Err(e) => {
                        activity.title("Couldn't send your suggestion");
                        activity.fail(e, &[Action::Retry, Action::CopyDetails]);
                        self.failed = Some((activity.id(), draft));
                    }
                }
            }
        }
        if let Some((id, draft)) = self.failed.clone() {
            if notices.take_action(id) == Some(Action::Retry) {
                self.failed = None;
                self.start_sending(ctx, notices, draft);
            }
        }
    }

    /// While the screen shows: takes what arrived, and reads the roadmap (and the player's
    /// suggestions) when due.
    fn poll(&mut self, ctx: &egui::Context, identity: bool) {
        if !self.cache_read {
            // What was read before, until the server answers.
            self.cache_read = true;
            if let Some((roadmap, at)) = setup::app_data_dir().and_then(|d| setup::roadmap::read_cache(&d)) {
                self.shown = Some((roadmap, at, false));
            }
        }
        match self.loading.poll() {
            Some(Ok(roadmap)) => {
                self.shown = Some((roadmap, identity::now(), true));
                self.error = None;
            }
            Some(Err(e)) => self.error = Some(e),
            None => {}
        }
        if !self.loading.running() && self.loaded_at.is_none_or(|t| t.elapsed() >= REFRESH_EVERY) {
            self.loaded_at = Some(Instant::now());
            self.loading.start(ctx, fetch_roadmap);
        }
        match self.fetching_mine.poll() {
            Some(Ok(mine)) => {
                self.mine = Some(mine);
                self.mine_error = None;
            }
            Some(Err(e)) => self.mine_error = Some(e),
            None => {}
        }
        // Asked when the screen opens (again after a while), once there's an identity to
        // sign with.
        if identity && (!self.mine_asked || self.mine_at.is_some_and(|t| t.elapsed() >= REFRESH_EVERY)) {
            self.ask_mine(ctx);
        }
    }

    fn ask_mine(&mut self, ctx: &egui::Context) {
        self.mine_asked = true;
        self.mine_at = Some(Instant::now());
        if !self.fetching_mine.running() {
            self.fetching_mine.start(ctx, fetch_mine);
        }
    }

    fn start_sending(&mut self, ctx: &egui::Context, notices: &mut Notices, draft: Draft) {
        let activity = notices.start(ctx, "Sending your suggestion");
        activity.step(format!("\"{}\"", hooks_config::text::clip(&draft.title, 40)));
        let sending = draft.clone();
        self.sending.start(ctx, move || send(&sending));
        self.sent = Some((activity, draft));
    }
}

/// The Roadmap screen.
pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (page, game, notices) = app.roadmap_mut();
    let profile = game.as_ref().and_then(|g| g.cfg.current_profile().cloned());
    let identity = has_identity();
    page.poll(&ctx, identity);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        theme::page().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(theme::display("Roadmap", 32.0));
            let clock = setup::clock::Clock::local();
            let source = match &page.shown {
                Some((_, at, false)) => format!("What's being built for 5th Echelon Enhanced, as of {}.", clock.date(*at)),
                _ => "What's being built for 5th Echelon Enhanced, and what's next.".into(),
            };
            ui.label(RichText::new(source).color(theme::SOFT));
            ui.add_space(14.0);
            lanes(page, ui);
            ui.add_space(26.0);
            form(page, profile.as_ref(), identity, notices, &ctx, ui);
            ui.add_space(26.0);
            mine(page, identity, &clock, ui);
        });
    });
}

/// The lanes as columns of cards (Shipping, Next, Later), and Requested below when it has
/// anything public.
fn lanes(page: &Roadmap, ui: &mut egui::Ui) {
    let Some((roadmap, _, _)) = &page.shown else {
        if page.loading.running() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted("Reading the roadmap…"));
            });
        } else if let Some(e) = &page.error {
            ui.label(theme::muted(e.as_str()));
        }
        return;
    };
    if let Some(e) = &page.error {
        ui.label(theme::muted(format!("{e} This is the roadmap as last read.")).small());
        ui.add_space(6.0);
    }
    let (requested, main): (Vec<_>, Vec<_>) = roadmap.lanes.iter().partition(|l| l.id == "requested");
    if main.is_empty() && requested.iter().all(|l| l.items.is_empty()) {
        ui.label(theme::muted("Nothing on the roadmap yet."));
        return;
    }
    let gap = 16.0;
    let columns = main.len().clamp(1, 3);
    let width = ((ui.available_width() - gap * (columns as f32 - 1.0)) / columns as f32).floor();
    for row in main.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for lane in row {
                ui.allocate_ui_with_layout(egui::vec2(width, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    ui.set_width(width);
                    lane_header(ui, lane);
                    if lane.items.is_empty() {
                        ui.label(theme::muted("Nothing here yet."));
                    }
                    for item in &lane.items {
                        item_card(ui, item, lane.id == "shipping");
                        ui.add_space(8.0);
                    }
                });
            }
        });
        ui.add_space(gap);
    }
    // Asked for by players: in a full-width band of the same cards.
    for lane in requested.into_iter().filter(|l| !l.items.is_empty()) {
        lane_header(ui, lane);
        for row in lane.items.chunks(3) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let width = ((ui.available_width() - 2.0 * gap) / 3.0).floor();
                for item in row {
                    ui.allocate_ui_with_layout(egui::vec2(width, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(width);
                        item_card(ui, item, false);
                    });
                }
            });
            ui.add_space(8.0);
        }
    }
}

/// "SHIPPING  0.4.2  3": the lane's name, its release, and how many items.
fn lane_header(ui: &mut egui::Ui, lane: &setup::roadmap::Lane) {
    ui.horizontal(|ui| {
        ui.label(theme::caps(&lane.title));
        if !lane.release.is_empty() {
            ui.label(RichText::new(&lane.release).monospace().size(12.5).color(theme::ACCENT));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::muted(lane.items.len().to_string()).small());
        });
    });
    ui.add_space(4.0);
}

/// One item: its title and status, what it is, and its tags.
fn item_card(ui: &mut egui::Ui, item: &setup::roadmap::Item, shipping: bool) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, if shipping { theme::ACCENT.linear_multiply(0.35) } else { theme::LINE }))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.add(egui::Label::new(RichText::new(&item.title).family(theme::strong()).size(15.0)).wrap());
            if !item.body.is_empty() {
                ui.add(egui::Label::new(RichText::new(&item.body).color(theme::SOFT).size(13.5)).wrap());
            }
            if !item.status.is_empty() || !item.tags.is_empty() {
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if !item.status.is_empty() {
                        chip(ui, &item.status, if shipping { theme::ACCENT } else { theme::FG });
                    }
                    for tag in &item.tags {
                        chip(ui, tag, theme::MUTED);
                    }
                });
            }
        });
}

/// A small outlined label.
fn chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.45)))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        });
}

/// "Suggest a feature or improvement": what it's about, a title, the details, and Send.
fn form(page: &mut Roadmap, profile: Option<&setup::config::Profile>, identity: bool, notices: &mut Notices, ctx: &egui::Context, ui: &mut egui::Ui) {
    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(theme::heading("Suggest a feature or improvement"));
        ui.label(theme::muted("It goes to the project's admins, with your name and server. You'll see their answer below."));
        ui.add_space(10.0);
        if !identity {
            ui.label(RichText::new("Connect to a server first: suggestions are signed with the identity the launcher makes then.").color(theme::SOFT));
            return;
        }
        ui.label(theme::caps("About"));
        let mut area = page.area.min(setup::roadmap::AREAS.len() - 1);
        let options: Vec<(usize, &str)> = setup::roadmap::AREAS.iter().copied().enumerate().collect();
        segmented_small(ui, &mut area, &options);
        page.area = area;
        ui.add_space(8.0);
        ui.label(theme::caps("Title"));
        ui.add(
            egui::TextEdit::singleline(&mut page.title)
                .hint_text("In a few words")
                .char_limit(setup::roadmap::TITLE_MAX)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(theme::caps("Details"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(theme::muted(format!("{}/{}", page.text.chars().count(), setup::roadmap::TEXT_MAX)).small());
            });
        });
        ui.add(
            egui::TextEdit::multiline(&mut page.text)
                .hint_text("What would you like, and what would it help with?")
                .char_limit(setup::roadmap::TEXT_MAX)
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        let checked = setup::roadmap::check(setup::roadmap::AREAS[page.area], &page.title, &page.text);
        let sending = page.sending.running();
        ui.horizontal(|ui| {
            let send = ui.add_enabled(checked.is_ok() && !sending, theme::primary("Send"));
            if sending {
                ui.spinner();
            } else if let Err(why) = &checked {
                // Said once there's something typed: an empty form needs no telling off.
                if !page.title.trim().is_empty() || !page.text.trim().is_empty() {
                    ui.label(theme::muted(why.as_str()));
                }
            }
            if send.clicked() {
                if let Ok((title, text)) = checked.clone() {
                    let draft = Draft {
                        area: setup::roadmap::AREAS[page.area].to_string(),
                        title,
                        text,
                        name: profile.map(|p| p.user.username.clone()).unwrap_or_default(),
                        server: profile.map(|p| p.server.clone()).filter(|s| !s.is_empty()),
                    };
                    page.start_sending(ctx, notices, draft);
                }
            }
        });
    });
}

/// Like `theme::segmented`, with buttons as wide as their words (five of them fit a row).
fn segmented_small(ui: &mut egui::Ui, value: &mut usize, options: &[(usize, &str)]) {
    egui::Frame::new()
        .fill(theme::SUNKEN)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (option, label) in options {
                    let chosen = *value == *option;
                    let text = RichText::new(*label).color(if chosen { theme::FG } else { theme::MUTED });
                    let text = if chosen { text.family(theme::strong()) } else { text };
                    let button = egui::Button::new(text)
                        .fill(if chosen { theme::CONTROL } else { egui::Color32::TRANSPARENT })
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(8)
                        .min_size(egui::vec2(0.0, 34.0));
                    if ui.add(button).clicked() {
                        *value = *option;
                    }
                }
            });
        });
}

/// "Your suggestions": each with its status and the admins' reply.
fn mine(page: &mut Roadmap, identity: bool, clock: &setup::clock::Clock, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(theme::heading("Your suggestions"));
        if page.fetching_mine.running() {
            ui.spinner();
        }
    });
    ui.add_space(6.0);
    if let Some(e) = &page.mine_error {
        ui.label(theme::muted(e.as_str()));
    }
    let Some(list) = &page.mine else {
        if !identity {
            ui.label(theme::muted("None yet."));
        }
        return;
    };
    if list.is_empty() {
        ui.label(theme::muted("None yet. What you send shows here, with the admins' answer."));
    }
    for s in list {
        egui::Frame::new()
            .fill(theme::SUNKEN)
            .stroke(egui::Stroke::new(1.0, theme::LINE))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&s.title).family(theme::strong()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (word, color) = match setup::roadmap::status_word(&s.status) {
                            "Planned" => ("Planned", theme::ACCENT),
                            "Done" => ("Done", theme::OK),
                            "Declined" => ("Declined", theme::MUTED),
                            word => (word, theme::FG),
                        };
                        chip(ui, word, color);
                    });
                });
                let when = if s.created_at > 0 { format!(" · {}", clock.date(s.created_at)) } else { String::new() };
                ui.label(theme::muted(format!("{}{when}", s.area)).small());
                if !s.reply.is_empty() {
                    ui.add_space(4.0);
                    egui::Frame::new()
                        .fill(theme::ACCENT.linear_multiply(0.06))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(theme::caps("Reply"));
                            ui.add(egui::Label::new(RichText::new(&s.reply).color(theme::SOFT).size(14.0)).wrap());
                        });
                }
            });
        ui.add_space(8.0);
    }
}
