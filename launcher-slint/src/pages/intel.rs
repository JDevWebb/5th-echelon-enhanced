//! Intel (the Tauri launcher's Intel.vue): the server's briefings, and on the community network
//! what's being built, with suggestions signed by the player's identity.

use super::*;
use crate::commands::Launcher;
use crate::intel::Intel;
use crate::tauri::Manager;

#[derive(Default)]
pub struct IntelPage {
    tab: usize,
    open: usize,
    area: usize,
    title: String,
    text: String,
    sent: bool,
    mine: Option<Result<Vec<serde_json::Value>, String>>,
}

impl Screen for IntelPage {
    fn live_edits(&self) -> bool {
        true
    }

    fn view(&mut self, app: &AppHandle) -> PageView {
        if crate::intel::MINE_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.mine = None;
        }
        let core = app.state::<Launcher>().0.clone();
        let info = crate::core::info(&core);
        let mut r = Rows::default();
        if !info.community {
            self.tab = 0;
        }
        let mut tabs = vec![("news".to_string(), "Briefings".to_string())];
        if info.community {
            tabs.push(("roadmap".into(), "Roadmap".into()));
            r.add(segments("tab", "", &tabs, &tabs[self.tab.min(tabs.len() - 1)].0));
        }
        let kicker = if self.tab == 1 { "What's being built" } else { "From your server" };
        if self.tab == 0 {
            if info.game_dir.is_none() {
                r.add(note("Find the game first (Play)."));
            } else if let Some(server) = &info.server {
                r.add(note(format!("From {server}, as the game's news screen shows it")));
                let news = crate::core::news_items(&core);
                if news.is_empty() {
                    r.add(mark("No briefings", "This server hasn't any news right now."));
                }
                self.open = self.open.min(news.len().saturating_sub(1));
                if news.len() > 1 {
                    r.card("Briefings");
                    for (i, (t, ..)) in news.iter().enumerate() {
                        r.add(
                            item(t.clone(), "").on(i == self.open).button(
                                super::b(format!("open:{i}"), if i == self.open { "Reading" } else { "Read" })
                                    .kind("fix")
                                    .enabled(i != self.open),
                            ),
                        );
                    }
                }
                if let Some((t, body, link)) = news.get(self.open) {
                    r.card(t);
                    r.add(note(format!("Briefing {} of {}", self.open + 1, news.len())).tone("accent"));
                    r.add_if(!body.is_empty(), || text(body.clone()));
                    // Where the server's link goes, said on the button before it's opened.
                    if let Some(host) = crate::core::link_host(link).filter(|_| link.starts_with("https://")) {
                        r.add(buttons(vec![super::b(format!("link:{link}"), format!("Read more on {host}")).kind("link")]));
                    }
                }
            } else {
                r.add(note("Choose a server first: its news shows here."));
            }
        } else {
            let road = json(&crate::intel::roadmap(app.state::<Intel>()));
            let as_of = st(&road, "as_of");
            r.add(note(if as_of.is_empty() {
                "What's being built for 5th Echelon Enhanced, and what's next.".to_string()
            } else {
                format!("What's being built for 5th Echelon Enhanced, as of {as_of}.")
            }));
            let lanes = arr(&road, "lanes");
            let error = st(&road, "error");
            if !error.is_empty() {
                r.add(note(format!("{error}{}", if lanes.is_empty() { "" } else { " This is the roadmap as last read." })));
            }
            for lane in lanes {
                let items = arr(lane, "items");
                if st(lane, "id") == "requested" && items.is_empty() {
                    continue;
                }
                let release = st(lane, "release");
                r.card(&format!(
                    "{}{} · {}",
                    st(lane, "title"),
                    if release.is_empty() { String::new() } else { format!(" ({release})") },
                    items.len()
                ));
                if items.is_empty() {
                    r.add(note("Nothing here yet."));
                }
                for it in items {
                    let mut chips: Vec<String> = Vec::new();
                    if !st(it, "status").is_empty() {
                        chips.push(st(it, "status"));
                    }
                    chips.extend(arr(it, "tags").iter().filter_map(|t| t.as_str().map(String::from)));
                    r.add(
                        item(st(it, "title"), st(it, "body"))
                            .text(chips.join(" · "))
                            .tone(if st(lane, "id") == "shipping" { "accent" } else { "" }),
                    );
                }
            }
            let form = json(&crate::intel::suggestion_form());
            r.card("Suggest a feature or improvement");
            r.add(note("It goes to the project's admins, with your name and server. You'll see their answer below."));
            if !bo(&form, "identity") {
                r.add(note("Connect to a server first: suggestions are signed with the identity the launcher makes then."));
            } else {
                let areas: Vec<(String, String)> = arr(&form, "areas").iter().filter_map(|a| a.as_str()).map(|a| (a.to_string(), a.to_string())).collect();
                let current = areas.get(self.area).map(|a| a.0.clone()).unwrap_or_default();
                r.add(segments("area", "About", &areas, &current));
                r.add(field("title", "Title", self.title.clone()).placeholder("In a few words"));
                r.add(area("text", "Details", self.text.clone()).placeholder("What would you like, and what would it help with?"));
                r.add(note(format!("{}/{}", self.text.chars().count(), num(&form, "text_max"))));
                let why = crate::intel::check_suggestion(self.area, self.title.clone(), self.text.clone());
                r.add(buttons(vec![super::b("send", "Send").kind("primary").enabled(why.is_none())]));
                if let Some(why) = why.filter(|_| !self.title.trim().is_empty() || !self.text.trim().is_empty()) {
                    r.add(note(why));
                }
                r.add_if(self.sent, || accent("Sent: it's in the activity bar, and below once the admins have it."));
            }
            r.card("Your suggestions");
            if self.mine.is_none() {
                self.mine = Some(crate::intel::my_suggestions().map(|list| list.iter().map(json).collect()));
            }
            match &self.mine {
                Some(Err(e)) => {
                    r.add(note(e.clone()));
                }
                Some(Ok(list)) if list.is_empty() => {
                    r.add(note("None yet. What you send shows here, with the admins' answer."));
                }
                Some(Ok(list)) => {
                    for s in list {
                        let reply = st(s, "reply");
                        r.add(item(st(s, "title"), st(s, "line")).text(if reply.is_empty() {
                            st(s, "status")
                        } else {
                            format!("{} · Reply: {reply}", st(s, "status"))
                        }));
                    }
                }
                None => {}
            }
        }
        PageView {
            kicker: kicker.into(),
            title: "Intel".into(),
            rows: r.done(),
            ..Default::default()
        }
    }

    fn chosen(&mut self, _app: &AppHandle, id: &str, index: usize) {
        match id {
            "tab" => {
                self.tab = index.min(1);
                self.mine = None;
            }
            "area" => self.area = index,
            _ => {}
        }
    }

    fn edited(&mut self, id: &str, text: &str) {
        self.sent = false;
        match id {
            "title" => self.title = text.into(),
            "text" => self.text = text.into(),
            _ => {}
        }
    }

    fn accepted(&mut self, _app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
    }

    fn pressed(&mut self, app: &AppHandle, id: &str) {
        if let Some(i) = id.strip_prefix("open:") {
            self.open = i.parse().unwrap_or(0);
        } else if let Some(link) = id.strip_prefix("link:") {
            let _ = crate::core::open_url(link);
        } else if id == "send" && crate::intel::suggest(app.clone(), self.area, self.title.clone(), self.text.clone()).is_ok() {
            self.title.clear();
            self.text.clear();
            self.sent = true;
            self.mine = None;
        }
    }
}
