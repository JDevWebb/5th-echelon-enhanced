//! The launcher's other screens (everything but Play and Matches) and its dialogs, as rows
//! (ui/rows.slint draws them): each page builds what it shows from the command modules
//! (settings.rs, chat.rs and the others, as the Tauri pages called them) and answers what the
//! player does. Built and answered on worker threads; the window only draws.

use serde_json::Value;

use crate::tauri::AppHandle;

pub mod chat;
pub mod dialogs;
pub mod friends;
pub mod host;
pub mod intel;
pub mod servers;
pub mod settings;
pub mod setup;
pub mod support;

/// A row, as the window draws it (ui/rows.slint's `Row`), in plain Rust (sent between threads).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct R {
    pub kind: &'static str,
    pub id: String,
    pub label: String,
    pub text: String,
    pub sub: String,
    pub tone: &'static str,
    pub on: bool,
    pub enabled: bool,
    pub options: Vec<String>,
    pub current: i32,
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub placeholder: String,
    pub secret: bool,
    pub buttons: Vec<B>,
    pub edge: i32,
}

/// A button on a row: its action's id, its label, its kind ("primary", "ghost", "plain",
/// "danger", "fix", "link"), and whether it can be pressed.
#[derive(Debug, Clone, PartialEq)]
pub struct B {
    pub id: String,
    pub label: String,
    pub kind: &'static str,
    pub enabled: bool,
}

pub fn b(id: impl Into<String>, label: impl Into<String>) -> B {
    B {
        id: id.into(),
        label: label.into(),
        kind: "plain",
        enabled: true,
    }
}

impl B {
    pub fn kind(mut self, kind: &'static str) -> Self {
        self.kind = kind;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

impl R {
    fn new(kind: &'static str) -> Self {
        R {
            kind,
            enabled: true,
            ..Default::default()
        }
    }
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
    pub fn sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = sub.into();
        self
    }
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }
    pub fn tone(mut self, tone: &'static str) -> Self {
        self.tone = tone;
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }
    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }
    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
    pub fn buttons(mut self, buttons: Vec<B>) -> Self {
        self.buttons = buttons;
        self
    }
    pub fn button(mut self, button: B) -> Self {
        self.buttons.push(button);
        self
    }
    /// A text area's height in pixels.
    pub fn lines_height(mut self, px: f32) -> Self {
        self.value = px;
        self
    }
}

pub fn title(label: impl Into<String>) -> R {
    R {
        label: label.into(),
        ..R::new("title")
    }
}
pub fn text(t: impl Into<String>) -> R {
    R::new("text").text(t)
}
pub fn bold(t: impl Into<String>) -> R {
    R::new("text").text(t).on(true)
}
pub fn note(t: impl Into<String>) -> R {
    R::new("note").text(t)
}
pub fn warn(t: impl Into<String>) -> R {
    R::new("note").text(t).tone("warn")
}
pub fn bad(t: impl Into<String>) -> R {
    R::new("note").text(t).tone("bad")
}
pub fn accent(t: impl Into<String>) -> R {
    R::new("note").text(t).tone("accent")
}
pub fn code(t: impl Into<String>) -> R {
    R::new("code").text(t)
}
pub fn switch(id: impl Into<String>, label: impl Into<String>, on: bool) -> R {
    R {
        label: label.into(),
        ..R::new("switch").id(id).on(on)
    }
}
/// A drop-down: the options' ids and labels, and the current one's id.
pub fn choice(id: impl Into<String>, label: impl Into<String>, options: &[(String, String)], current: &str) -> R {
    R {
        label: label.into(),
        options: options.iter().map(|(_, l)| l.clone()).collect(),
        current: options.iter().position(|(i, _)| i == current).unwrap_or(0) as i32,
        ..R::new("choice").id(id)
    }
}
/// Side-by-side choices (a few), as `choice`; none shows chosen when `current` isn't one.
pub fn segments(id: impl Into<String>, label: impl Into<String>, options: &[(String, String)], current: &str) -> R {
    let index = options.iter().position(|(i, _)| i == current).map_or(-1, |i| i as i32);
    R {
        kind: "segments",
        current: index,
        ..choice(id, label, options, current)
    }
}
pub fn slider(id: impl Into<String>, label: impl Into<String>, value: f32, min: f32, max: f32, shown: impl Into<String>) -> R {
    R {
        label: label.into(),
        value,
        min,
        max,
        ..R::new("slider").id(id).text(shown)
    }
}
pub fn field(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> R {
    R {
        label: label.into(),
        ..R::new("field").id(id).text(value)
    }
}
pub fn area(id: impl Into<String>, label: impl Into<String>, value: impl Into<String>) -> R {
    R {
        label: label.into(),
        ..R::new("area").id(id).text(value)
    }
}
pub fn buttons(list: Vec<B>) -> R {
    R::new("buttons").buttons(list)
}
pub fn item(label: impl Into<String>, sub: impl Into<String>) -> R {
    R {
        label: label.into(),
        ..R::new("item").sub(sub)
    }
}
pub fn result(label: impl Into<String>, tone: &'static str, sub: impl Into<String>) -> R {
    R {
        label: label.into(),
        ..R::new("result").tone(tone).sub(sub)
    }
}
pub fn message(who: impl Into<String>, what: impl Into<String>, when: impl Into<String>) -> R {
    R {
        label: who.into(),
        ..R::new("message").text(what).sub(when)
    }
}
pub fn working(t: impl Into<String>) -> R {
    R::new("working").text(t)
}
pub fn rule() -> R {
    R::new("rule")
}
#[allow(dead_code)]
pub fn space() -> R {
    R::new("space")
}
/// The brand's mark over a heading and a line: an empty page (or an empty list), with what to
/// do about it as its buttons.
pub fn mark(heading: impl Into<String>, line: impl Into<String>) -> R {
    R {
        label: heading.into(),
        ..R::new("mark").text(line)
    }
}

/// A page's rows, gathered card by card.
#[derive(Default)]
pub struct Rows {
    rows: Vec<(usize, bool, R)>,
    group: usize,
    in_card: bool,
}

impl Rows {
    /// A new card, with its title (none when empty).
    pub fn card(&mut self, heading: &str) -> &mut Self {
        self.group += 1;
        self.in_card = true;
        if !heading.is_empty() {
            self.add(title(heading));
        }
        self
    }
    /// Rows outside any card from here.
    pub fn out(&mut self) -> &mut Self {
        self.group += 1;
        self.in_card = false;
        self
    }
    pub fn add(&mut self, row: R) -> &mut Self {
        self.rows.push((self.group, self.in_card, row));
        self
    }
    pub fn add_if(&mut self, yes: bool, row: impl FnOnce() -> R) -> &mut Self {
        if yes {
            self.add(row());
        }
        self
    }
    /// The rows, each knowing its place in its card, a space between groups.
    pub fn done(self) -> Vec<R> {
        let mut out = Vec::new();
        let mut i = 0;
        let rows = self.rows;
        while i < rows.len() {
            let (group, carded) = (rows[i].0, rows[i].1);
            let end = rows[i..].iter().position(|(g, ..)| *g != group).map_or(rows.len(), |n| i + n);
            if !out.is_empty() {
                out.push(R::new("space"));
            }
            let n = end - i;
            for (k, (_, _, r)) in rows[i..end].iter().enumerate() {
                let edge = match (carded, n, k) {
                    (false, ..) => 0,
                    (true, 1, _) => 4,
                    (true, _, 0) => 1,
                    (true, n, k) if k == n - 1 => 3,
                    _ => 2,
                };
                out.push(R { edge, ..r.clone() });
            }
            i = end;
        }
        out
    }
}

/// What a page shows: its kicker and title (or, with sections, the section's name as the
/// summary), its sections down the side, and its rows.
#[derive(Debug, Clone, Default)]
pub struct PageView {
    pub kicker: String,
    pub title: String,
    pub summary: String,
    pub sections: Vec<(String, String)>,
    pub section: String,
    pub rows: Vec<R>,
}

/// A page: what it shows, and what the player does on it. Every call may block (the network):
/// they run on worker threads.
pub trait Screen: Send {
    fn view(&mut self, app: &AppHandle) -> PageView;
    fn section(&mut self, _id: &str) {}
    fn toggled(&mut self, _app: &AppHandle, _id: &str, _on: bool) {}
    fn chosen(&mut self, _app: &AppHandle, _id: &str, _index: usize) {}
    fn slid(&mut self, _app: &AppHandle, _id: &str, _value: f32) {}
    /// A field's text as typed (kept, not acted on).
    fn edited(&mut self, _id: &str, _text: &str) {}
    fn accepted(&mut self, app: &AppHandle, id: &str, text: &str) {
        self.edited(id, text);
        self.pressed(app, id);
    }
    fn pressed(&mut self, _app: &AppHandle, _id: &str) {}
    /// Whether typing in a field changes what the page shows (a Send button, a count): it's
    /// built again after each edit then.
    fn live_edits(&self) -> bool {
        false
    }
    /// Asked again on its own every so many seconds while shown (0: never).
    fn refresh_every(&self) -> u64 {
        0
    }
}

/// A JSON value's field as text ("" when it's missing or null).
pub fn st(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}
pub fn bo(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}
pub fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}
pub fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}
/// A value as JSON (the command modules' views are Serialize, as the Tauri page got them).
pub fn json<T: serde::Serialize>(t: &T) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}
/// A list of (id, label) pairs from JSON `[[id, label], …]`.
pub fn pairs(v: &[Value]) -> Vec<(String, String)> {
    v.iter().filter_map(|p| Some((p.get(0)?.as_str()?.to_string(), p.get(1)?.as_str()?.to_string()))).collect()
}
#[allow(dead_code)]
pub fn same(list: &[&str]) -> Vec<(String, String)> {
    list.iter().map(|s| (s.to_string(), s.to_string())).collect()
}
