//! The standby coordinator: each server of a failover group runs one
//! (`coordinator standby`, as root), and the coordinator runs on whichever
//! of them its record points at (docs/failover.md).
//!
//! The coordinator's record (e.g. play.scbl.example.net) is proxied through
//! Cloudflare, so it changes for everyone at once, and it's what says where
//! the coordinator is. Every 15 seconds each standby reads it (Cloudflare's
//! API) and:
//!
//! * where it points here: makes sure the coordinator runs, and that the
//!   other records (the admin UI's) point here too;
//! * where it points at another server: makes sure the coordinator doesn't
//!   run here (one that was replaced stands down when it's back), and asks
//!   for the coordinator through Cloudflare. When Cloudflare answers with an
//!   error from its server (down, or its coordinator is) for long enough, it
//!   takes over: after 3 minutes for the first in line, 3 more for each one
//!   after, so the servers don't race. First it asks the other servers
//!   whether they reach the coordinator (one that does stops it); then it
//!   restores the coordinator from the live backup of the one it replaces,
//!   starts it, looks at the record once more and points it here.
//!
//! No answer at all (this server's own network) changes nothing, and nor
//! does an answer that isn't an error from the coordinator's server (a
//! Cloudflare challenge, say). Coming back isn't automatic: a server that
//! was replaced stays a standby until an admin moves the coordinator back
//! (`coordinator standby --take-over` there).

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;

/// How often a standby looks.
pub const EVERY: Duration = Duration::from_secs(15);
/// How long the first in line waits with the coordinator down; each after it waits this
/// much longer.
pub const WAIT: Duration = Duration::from_secs(180);
/// After a takeover that didn't work, the next try waits this long.
pub const RETRY_AFTER: Duration = Duration::from_secs(300);
/// How often the coordinator's machine checks the records that follow it (the admin UI's):
/// Cloudflare's API allows a user 1,200 requests in 5 minutes, for everything they do.
pub const FOLLOW_EVERY: Duration = Duration::from_secs(60);
/// A server reaches the coordinator if its last heartbeat (every 30 s) is this recent.
pub const REACHED_WITHIN: u64 = 90;
/// What the coordinator's `/v1/info` calls itself.
const COORDINATOR_NAME: &str = "5th Echelon coordinator";

/// A server of the group, in the order they take over.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    /// Its backup name (docs/backups.md): the live copy a standby restores from is
    /// `live/<name>/coordinator`.
    pub name: String,
    /// Its game server's host name (DNS only, not proxied), for its `/api/info`.
    pub host: String,
}

/// `/etc/5th-echelon/standby.conf`, written by install-server.sh.
#[derive(Debug, Clone)]
pub struct Config {
    /// The coordinator's record first, then any that follow it (the admin UI's).
    pub records: Vec<String>,
    /// This server's name in `servers`.
    pub me: String,
    /// This server's public address: what the records point at when the coordinator runs here.
    pub address: IpAddr,
    pub servers: Vec<Server>,
    /// The file with CLOUDFLARE_API_TOKEN and CLOUDFLARE_ZONE_ID (root's, mode 600).
    pub secrets: PathBuf,
    pub state: PathBuf,
    /// The coordinator's service, and its address on this machine.
    pub service: String,
    pub local: String,
    /// backup.sh, which restores the coordinator (docs/backups.md).
    pub backup: PathBuf,
    /// The coordinator's database: never started without one (a new, empty one would
    /// have none of the servers' secrets).
    pub database: PathBuf,
    pub api: String,
    /// Where the coordinator is asked for, through Cloudflare (`https://<record>/v1/info`).
    pub probe: String,
    /// Peers' `/api/info` over this scheme: https (their certificates checked), so nobody
    /// on the way can say a server does or doesn't reach the coordinator.
    pub peer_scheme: String,
    /// How long the first in line waits (`wait SECONDS`; [`WAIT`] unless set).
    pub wait: Duration,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let (mut records, mut me, mut address, mut servers) = (Vec::new(), None, None, Vec::new());
        let mut set: HashMap<&str, String> = HashMap::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let words: Vec<&str> = line.split_whitespace().collect();
            let bad = || format!("line {}: {line}", n + 1);
            match words.as_slice() {
                ["record", name] if crate::valid_host(name) => records.push((*name).to_string()),
                ["me", name] => me = Some((*name).to_string()),
                ["address", ip] => address = Some(ip.parse::<IpAddr>().map_err(|_| bad())?),
                ["server", name, host] if valid_name(name) && crate::valid_host(host.split(':').next().unwrap_or_default()) => {
                    if servers.iter().any(|s: &Server| s.name == *name) {
                        return Err(format!("{} is in the list twice", name));
                    }
                    servers.push(Server {
                        name: (*name).to_string(),
                        host: (*host).to_string(),
                    });
                }
                [key @ ("secrets" | "state" | "service" | "local" | "backup" | "database" | "api" | "probe" | "peer-scheme" | "wait"), value] => {
                    set.insert(key, (*value).to_string());
                }
                _ => return Err(bad()),
            }
        }
        let me = me.ok_or("no `me` (this server's name)")?;
        if records.is_empty() {
            return Err("no `record` (the coordinator's host name)".into());
        }
        if servers.len() < 2 {
            return Err("a failover group is two servers or more".into());
        }
        if !servers.iter().any(|s| s.name == me) {
            return Err(format!("{me} (me) isn't one of the servers"));
        }
        let mut take = |key: &str, default: &str| set.remove(key).unwrap_or_else(|| default.to_string());
        Ok(Self {
            probe: take("probe", &format!("https://{}/v1/info", records[0])),
            records,
            me,
            address: address.ok_or("no `address` (this server's public address)")?,
            servers,
            secrets: take("secrets", "/etc/5th-echelon/standby.env").into(),
            state: take("state", "/var/lib/5th-echelon-standby").into(),
            service: take("service", "5th-echelon-coordinator"),
            local: take("local", "http://127.0.0.2:8700"),
            backup: take("backup", "/opt/5th-echelon/backup.sh").into(),
            database: take("database", "/var/lib/5th-echelon-coordinator/coordinator.db").into(),
            api: take("api", "https://api.cloudflare.com/client/v4"),
            peer_scheme: Some(take("peer-scheme", "https"))
                .filter(|s| s == "https" || s == "http")
                .ok_or("`peer-scheme` is https or http")?,
            wait: match set.remove("wait") {
                Some(secs) => Duration::from_secs(secs.parse::<u64>().ok().filter(|s| (10..=3600).contains(s)).ok_or("`wait` is 10 to 3600 seconds")?),
                None => WAIT,
            },
        })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn my_index(&self) -> usize {
        self.servers.iter().position(|s| s.name == self.me).unwrap_or(0)
    }

    /// Where this server is in line to replace `primary`: 1 for the first after it.
    pub fn rank(&self, primary: usize) -> u32 {
        let me = self.my_index();
        1 + self.servers.iter().enumerate().filter(|(i, _)| *i != primary && *i < me).count() as u32
    }

    /// How long the coordinator must have been down before this server replaces `primary`.
    pub fn wait(&self, primary: usize) -> Duration {
        self.wait * self.rank(primary)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The Cloudflare API token and zone, from the secrets file: root's, readable by nobody else.
pub fn secrets(path: &Path) -> Result<(String, String), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 {
            return Err(format!("{} must be a file only its owner can read (mode 600)", path.display()));
        }
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut values: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), value.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    let mut get = |key: &str| values.remove(key).filter(|v| !v.is_empty()).ok_or(format!("{key} isn't set in {}", path.display()));
    Ok((get("CLOUDFLARE_API_TOKEN")?, get("CLOUDFLARE_ZONE_ID")?))
}

/// Where a record points.
#[derive(Debug, Clone, PartialEq)]
pub enum Where {
    Here,
    /// At this server of the group.
    There(usize),
    /// At an address that's none of the group's (set by hand): left alone.
    Elsewhere(IpAddr),
}

/// What Cloudflare answers for the coordinator.
#[derive(Debug, Clone, PartialEq)]
pub enum Health {
    Up,
    /// An error from the coordinator's server (or from Cloudflare, not reaching it).
    Down(String),
    /// No answer at all, or one that says nothing about the coordinator.
    Unknown(String),
}

/// What a standby does next.
#[derive(Debug, PartialEq)]
pub enum Action {
    Nothing,
    /// Start the coordinator here (the record points here).
    Start,
    /// Stop the coordinator here (the record points elsewhere).
    StandDown,
    /// The coordinator is down; this server's turn comes after this long.
    Waiting(Duration),
    /// The coordinator has been down long enough: replace this server (if no other
    /// server still reaches it).
    Due(usize),
}

/// What a standby remembers between looks.
#[derive(Debug, Default)]
pub struct Watch {
    /// Since when the coordinator has been down, as Cloudflare answers.
    pub down_since: Option<Instant>,
    /// Whether the record last pointed here (kept in the state folder): a server that
    /// starts without Cloudflare's API answering starts the coordinator only if so.
    pub was_here: bool,
    pub retry_after: Option<Instant>,
}

impl Watch {
    /// The next step, from where the coordinator's record points (None: Cloudflare's API
    /// didn't answer), what Cloudflare answers for it, and whether it runs here.
    pub fn decide(&mut self, cfg: &Config, record: Option<&Where>, health: &Health, running: bool, now: Instant) -> Action {
        let Some(record) = record else {
            // Without the API, only what was so before: a coordinator that ran here starts.
            return if self.was_here && !running { Action::Start } else { Action::Nothing };
        };
        self.was_here = *record == Where::Here;
        match record {
            Where::Here => {
                self.down_since = None;
                if running {
                    Action::Nothing
                } else {
                    Action::Start
                }
            }
            _ if running => Action::StandDown,
            Where::Elsewhere(_) => {
                self.down_since = None;
                Action::Nothing
            }
            Where::There(primary) => {
                match health {
                    Health::Up => {
                        self.down_since = None;
                        return Action::Nothing;
                    }
                    // Neither starts nor resets the wait: this server's network says nothing
                    // about the coordinator.
                    Health::Unknown(_) => {}
                    Health::Down(_) => {
                        self.down_since.get_or_insert(now);
                    }
                }
                let Some(since) = self.down_since else {
                    return Action::Nothing;
                };
                if self.retry_after.is_some_and(|t| now < t) {
                    return Action::Waiting(self.retry_after.map_or(Duration::ZERO, |t| t - now));
                }
                let wait = cfg.wait(*primary);
                let down = now.saturating_duration_since(since);
                if down < wait {
                    Action::Waiting(wait - down)
                } else if matches!(health, Health::Down(_)) {
                    Action::Due(*primary)
                } else {
                    Action::Waiting(Duration::ZERO)
                }
            }
        }
    }
}

/// A DNS record, as Cloudflare's API has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub content: IpAddr,
}

/// Cloudflare's DNS API, for one zone.
pub struct Cloudflare {
    http: reqwest::Client,
    api: String,
    zone: String,
    token: String,
}

impl Cloudflare {
    pub fn new(http: reqwest::Client, api: &str, zone: &str, token: &str) -> Self {
        Self {
            http,
            api: api.trim_end_matches('/').to_string(),
            zone: zone.to_string(),
            token: token.to_string(),
        }
    }

    async fn call(&self, request: reqwest::RequestBuilder) -> Result<Value, String> {
        let response = request
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| format!("Cloudflare's API: {e}"))?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|e| format!("Cloudflare's API ({status}): {e}"))?;
        if !status.is_success() || body["success"] != Value::Bool(true) {
            let errors: Vec<String> = body["errors"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|e| format!("{} {}", e["code"], e["message"].as_str().unwrap_or_default()))
                .collect();
            return Err(format!("Cloudflare's API ({status}): {}", errors.join("; ")));
        }
        Ok(body)
    }

    /// The name's record: one A record, and no AAAA or CNAME beside it (Cloudflare would
    /// send some visitors there, whatever the A record says).
    pub async fn record(&self, name: &str) -> Result<Record, String> {
        let url = format!("{}/zones/{}/dns_records", self.api, self.zone);
        let body = self.call(self.http.get(url).query(&[("name", name), ("per_page", "50")])).await?;
        let list = body["result"].as_array().cloned().unwrap_or_default();
        let of = |kind: &str| list.iter().filter(|r| r["type"] == kind).collect::<Vec<_>>();
        let (a, others) = (of("A"), of("AAAA").len() + of("CNAME").len());
        if a.len() != 1 || others > 0 {
            return Err(format!(
                "{name} has {} A record(s) and {others} AAAA or CNAME: failover needs exactly one A record",
                a.len()
            ));
        }
        let content = a[0]["content"].as_str().and_then(|c| c.parse().ok()).ok_or(format!("{name}'s record has no address"))?;
        let id = a[0]["id"].as_str().filter(|id| !id.is_empty()).ok_or(format!("{name}'s record has no id"))?;
        Ok(Record {
            id: id.to_string(),
            name: name.to_string(),
            content,
        })
    }

    /// Points the record at `to` (proxied as it was).
    pub async fn point(&self, record: &Record, to: IpAddr) -> Result<(), String> {
        let url = format!("{}/zones/{}/dns_records/{}", self.api, self.zone, record.id);
        self.call(self.http.patch(url).json(&serde_json::json!({ "content": to.to_string() }))).await.map(drop)
    }
}

/// Asks for the coordinator at `url` (through Cloudflare).
pub async fn probe(http: &reqwest::Client, url: &str) -> Health {
    // Longer than Cloudflare takes to give up on a server that doesn't answer (522).
    let response = match http.get(url).timeout(Duration::from_secs(30)).send().await {
        Ok(r) => r,
        Err(e) => return Health::Unknown(e.without_url().to_string()),
    };
    let status = response.status();
    // Cloudflare's own challenge or block, whatever its status: the probe was stopped on the
    // way, which says nothing about the coordinator.
    if response.headers().contains_key("cf-mitigated") {
        return Health::Unknown(format!("stopped by Cloudflare (HTTP {})", status.as_u16()));
    }
    if status.is_server_error() {
        return Health::Down(format!("HTTP {}", status.as_u16()));
    }
    if !status.is_success() {
        return Health::Unknown(format!("HTTP {}", status.as_u16()));
    }
    match response.json::<Value>().await {
        Ok(v) if v["name"] == COORDINATOR_NAME => Health::Up,
        // Something else answered for it (a page put up in front, a redirect, an answer cut
        // short): not a sign the coordinator is down, so not a reason to take over.
        _ => Health::Unknown(String::from("an answer that isn't the coordinator's")),
    }
}

/// Whether a server reaches the coordinator (its `/api/info`); None if it doesn't answer.
pub async fn reaches(http: &reqwest::Client, scheme: &str, host: &str) -> Option<bool> {
    let response = http.get(format!("{scheme}://{host}/api/info")).timeout(Duration::from_secs(10)).send().await.ok()?;
    let info: Value = response.json().await.ok()?;
    Some(info["coordinator_seen"].as_u64().is_some_and(|ago| ago <= REACHED_WITHIN))
}

/// What a standby does to this machine.
pub trait Machine: Send + Sync {
    fn running(&self) -> bool;
    fn start(&self) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    /// Restores the coordinator (its database, join token and reports) from `from`'s live
    /// backup, and starts it.
    fn restore(&self, from: &str) -> Result<(), String>;
}

/// systemd and backup.sh.
pub struct Systemd {
    pub service: String,
    pub backup: PathBuf,
}

fn run(command: &mut std::process::Command) -> Result<(), String> {
    let out = command.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    let tail: Vec<&str> = text.lines().rev().take(5).collect();
    Err(format!("{} ({})", tail.into_iter().rev().collect::<Vec<_>>().join(" / "), out.status))
}

impl Machine for Systemd {
    fn running(&self) -> bool {
        std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", &self.service])
            .status()
            .is_ok_and(|s| s.success())
    }

    fn start(&self) -> Result<(), String> {
        run(std::process::Command::new("systemctl").args(["start", &self.service]))
    }

    fn stop(&self) -> Result<(), String> {
        run(std::process::Command::new("systemctl").args(["stop", &self.service]))
    }

    fn restore(&self, from: &str) -> Result<(), String> {
        run(std::process::Command::new(&self.backup).args(["restore", "coordinator", "--from", from, "--with-files"]))
    }
}

/// A standby: its settings, Cloudflare, this machine, and what it remembers.
pub struct Standby {
    pub cfg: Config,
    pub cloudflare: Cloudflare,
    pub machine: Arc<dyn Machine>,
    pub http: reqwest::Client,
    pub watch: Watch,
    /// The group's servers' addresses, as last resolved.
    addresses: HashMap<String, Vec<IpAddr>>,
    /// What was last logged about the coordinator, so a change is logged once.
    said: String,
    /// When the records that follow the coordinator's were last checked.
    followed: Option<Instant>,
}

impl Standby {
    pub fn new(cfg: Config, cloudflare: Cloudflare, machine: Arc<dyn Machine>, http: reqwest::Client) -> Self {
        let was_here = std::fs::read_to_string(cfg.state.join("primary")).is_ok_and(|s| s.trim() == cfg.me);
        Self {
            cfg,
            cloudflare,
            machine,
            http,
            watch: Watch { was_here, ..Watch::default() },
            addresses: HashMap::new(),
            said: String::new(),
            followed: None,
        }
    }

    fn remember(&self, primary: &str) {
        let _ = std::fs::create_dir_all(&self.cfg.state);
        let _ = std::fs::write(self.cfg.state.join("primary"), format!("{primary}\n"));
    }

    fn say(&mut self, text: String) {
        if text != self.said {
            tracing::info!("{text}");
            self.said = text;
        }
    }

    /// Which server an address is: this one by its own address, the others by their host
    /// names (looked up each time; the last answer if a lookup fails).
    pub async fn where_is(&mut self, ip: IpAddr) -> Where {
        if ip == self.cfg.address {
            return Where::Here;
        }
        for (i, server) in self.cfg.servers.clone().into_iter().enumerate() {
            let host = if server.host.contains(':') {
                server.host.clone()
            } else {
                format!("{}:80", server.host)
            };
            if let Ok(found) = tokio::net::lookup_host(host).await {
                let ips: Vec<IpAddr> = found.map(|a| a.ip()).collect();
                if !ips.is_empty() {
                    self.addresses.insert(server.name.clone(), ips);
                }
            }
            if self.addresses.get(&server.name).is_some_and(|ips| ips.contains(&ip)) {
                return if server.name == self.cfg.me { Where::Here } else { Where::There(i) };
            }
        }
        Where::Elsewhere(ip)
    }

    async fn blocking<T: Send + 'static>(&self, job: impl FnOnce(&dyn Machine) -> T + Send + 'static) -> T {
        let machine = Arc::clone(&self.machine);
        tokio::task::spawn_blocking(move || job(machine.as_ref())).await.expect("a machine call panicked")
    }

    /// The other servers (not this one, not the coordinator's) that still reach the
    /// coordinator.
    pub async fn reached_by(&self, primary: usize) -> Vec<String> {
        let mut by = Vec::new();
        for (i, server) in self.cfg.servers.iter().enumerate() {
            if i != primary && server.name != self.cfg.me && reaches(&self.http, &self.cfg.peer_scheme, &server.host).await == Some(true) {
                by.push(server.name.clone());
            }
        }
        by
    }

    /// One look, and what came of it.
    pub async fn tick(&mut self) -> Action {
        let now = Instant::now();
        let record = match self.cloudflare.record(&self.cfg.records[0]).await {
            Ok(r) => Some(r),
            Err(e) => {
                self.say(format!("Can't read the coordinator's record: {e}"));
                None
            }
        };
        let place = match &record {
            Some(r) => Some(self.where_is(r.content).await),
            None => None,
        };
        let running = self.blocking(|m| m.running()).await;
        let health = match &place {
            Some(Where::There(_)) => probe(&self.http, &self.cfg.probe).await,
            _ => Health::Unknown(String::from("not asked")),
        };
        let action = self.watch.decide(&self.cfg, place.as_ref(), &health, running, now);
        match (&place, &action) {
            (Some(Where::Here), _) => {
                self.remember(&self.cfg.me.clone());
                self.say(String::from("The coordinator runs here"));
                if self.followed.is_none_or(|t| t.elapsed() >= FOLLOW_EVERY) {
                    self.follow().await;
                }
            }
            (Some(Where::There(i)), _) => {
                let name = self.cfg.servers[*i].name.clone();
                self.remember(&name);
                match &health {
                    Health::Up => self.say(format!("Standing by: the coordinator runs on {name}")),
                    Health::Down(why) => self.say(format!("The coordinator on {name} is down ({why})")),
                    Health::Unknown(why) => self.say(format!("Can't ask for the coordinator on {name}: {why}")),
                }
            }
            (Some(Where::Elsewhere(ip)), _) => self.say(format!("The coordinator's record points at {ip}, none of this group's servers: left alone")),
            (None, _) => {}
        }
        match &action {
            Action::Start if !self.cfg.database.exists() => self.say(format!(
                "The coordinator's record points here, but there's no {}: not starting a coordinator without its data (restore it: backup.sh restore coordinator --from NAME --with-files)",
                self.cfg.database.display()
            )),
            Action::Start => {
                tracing::info!("Starting the coordinator: its record points here");
                if let Err(e) = self.blocking(|m| m.start()).await {
                    tracing::error!("Couldn't start the coordinator: {e}");
                }
            }
            Action::StandDown => {
                tracing::warn!("Standing down: the coordinator's record points at another server, so the coordinator here stops");
                if let Err(e) = self.blocking(|m| m.stop()).await {
                    tracing::error!("Couldn't stop the coordinator: {e}");
                }
            }
            Action::Due(primary) => {
                let primary = *primary;
                let by = self.reached_by(primary).await;
                if by.is_empty() {
                    if let Err(e) = self.take_over(primary, true).await {
                        tracing::error!("Taking over didn't work: {e}; trying again in {} min", RETRY_AFTER.as_secs() / 60);
                        self.watch.retry_after = Some(Instant::now() + RETRY_AFTER);
                    }
                } else {
                    self.say(format!(
                        "The coordinator looks down from here, but {} still reach(es) it: not taking over",
                        by.join(", ")
                    ));
                }
            }
            Action::Nothing | Action::Waiting(_) => {}
        }
        action
    }

    /// Points the records that follow the coordinator's here.
    async fn follow(&mut self) {
        self.followed = Some(Instant::now());
        for name in self.cfg.records.clone().into_iter().skip(1) {
            match self.cloudflare.record(&name).await {
                Ok(r) if r.content == self.cfg.address => {}
                Ok(r) => match self.cloudflare.point(&r, self.cfg.address).await {
                    Ok(()) => tracing::info!("Pointed {name} here, with the coordinator"),
                    Err(e) => tracing::warn!("Couldn't point {name} here: {e}"),
                },
                Err(e) => self.say(format!("Can't read {name}'s record: {e}")),
            }
        }
    }

    /// Replaces the coordinator on `primary`: restores it here from that server's live
    /// backup, starts it, and points the records here if they still point there.
    /// `because_down`: it was found down (not `--take-over`), so it mustn't be answering again.
    pub async fn take_over(&mut self, primary: usize, because_down: bool) -> Result<(), String> {
        let from = self.cfg.servers[primary].name.clone();
        let still = |r: &Record, place: &Where| match place {
            Where::There(i) if *i == primary => Ok(()),
            _ => Err(format!("the record moved meanwhile (it points at {})", r.content)),
        };
        let record = self.cloudflare.record(&self.cfg.records[0]).await?;
        let place = self.where_is(record.content).await;
        still(&record, &place)?;
        tracing::warn!("Taking over from {from}: restoring the coordinator from its live backup");
        let restore_from = from.clone();
        self.blocking(move |m| m.restore(&restore_from)).await?;
        let mut answers = false;
        for _ in 0..30 {
            if self
                .http
                .get(format!("{}/v1/info", self.cfg.local))
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                answers = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        // A last look: another server may have taken over while this one restored.
        let record = self.cloudflare.record(&self.cfg.records[0]).await?;
        let place = self.where_is(record.content).await;
        let ready = if answers {
            still(&record, &place)
        } else {
            Err(String::from("the restored coordinator doesn't answer"))
        };
        // And the coordinator itself, which may have come back while this one restored: it
        // has taken writes since the backup, which moving the record here would lose.
        let ready = match ready {
            Ok(()) if because_down => match probe(&self.http, &self.cfg.probe).await {
                Health::Up => Err(format!("the coordinator on {from} answers again")),
                _ => Ok(()),
            },
            other => other,
        };
        if let Err(e) = ready {
            let _ = self.blocking(|m| m.stop()).await;
            return Err(e);
        }
        self.cloudflare.point(&record, self.cfg.address).await?;
        self.remember(&self.cfg.me.clone());
        self.watch.was_here = true;
        self.watch.down_since = None;
        tracing::warn!("Took over from {from}: {} points here now", record.name);
        self.follow().await;
        Ok(())
    }

    /// For `--status`: what this standby sees, as lines.
    pub async fn status(&mut self) -> Vec<String> {
        let mut lines = vec![format!(
            "This server: {} ({}), in a group of {}",
            self.cfg.me,
            self.cfg.address,
            self.cfg.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
        )];
        let running = self.blocking(|m| m.running()).await;
        lines.push(format!("Coordinator here: {}", if running { "running" } else { "stopped" }));
        match self.cloudflare.record(&self.cfg.records[0]).await {
            Err(e) => lines.push(format!("Record: {e}")),
            Ok(r) => match self.where_is(r.content).await {
                Where::Here => lines.push(format!("Record: {} points here", r.name)),
                Where::Elsewhere(ip) => lines.push(format!("Record: {} points at {ip}, none of the group's servers", r.name)),
                Where::There(i) => {
                    let name = self.cfg.servers[i].name.clone();
                    lines.push(format!("Record: {} points at {name}", r.name));
                    lines.push(format!("Through Cloudflare: {:?}", probe(&self.http, &self.cfg.probe).await));
                    lines.push(format!("This server takes over after {} s down", self.cfg.wait(i).as_secs()));
                }
            },
        }
        for server in &self.cfg.servers {
            if server.name != self.cfg.me {
                let reach = match reaches(&self.http, &self.cfg.peer_scheme, &server.host).await {
                    Some(true) => "reaches the coordinator",
                    Some(false) => "doesn't reach the coordinator (or says nothing)",
                    None => "doesn't answer",
                };
                lines.push(format!("{}: {reach}", server.name));
            }
        }
        lines
    }
}

/// `coordinator standby`: looks every 15 seconds until stopped; with `status`, once.
pub async fn main(config: &Path, status: bool, take_over: bool) -> eyre::Result<()> {
    let cfg = Config::load(config).map_err(|e| eyre::eyre!(e))?;
    let (token, zone) = secrets(&cfg.secrets).map_err(|e| eyre::eyre!(e))?;
    let http = reqwest::Client::builder().user_agent(concat!("5th-echelon-standby/", env!("FE_RELEASE"))).build()?;
    let cloudflare = Cloudflare::new(http.clone(), &cfg.api, &zone, &token);
    let machine = Arc::new(Systemd {
        service: cfg.service.clone(),
        backup: cfg.backup.clone(),
    });
    let mut standby = Standby::new(cfg, cloudflare, machine, http);
    if status {
        for line in standby.status().await {
            println!("{line}");
        }
        return Ok(());
    }
    if take_over {
        // The standby here would stop the restored coordinator before the record moves.
        let unit = "5th-echelon-standby";
        let paused = std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", unit])
            .status()
            .is_ok_and(|s| s.success());
        if paused {
            let _ = std::process::Command::new("systemctl").args(["stop", unit]).status();
        }
        let done = take_over_here(&mut standby).await;
        if paused {
            let _ = std::process::Command::new("systemctl").args(["start", unit]).status();
        }
        return done;
    }
    tracing::info!(
        "Standby for {} as {}: takes over after the coordinator is down {} s for each place in line",
        standby.cfg.records[0],
        standby.cfg.me,
        standby.cfg.wait.as_secs()
    );
    let mut ticks = tokio::time::interval(EVERY);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        standby.tick().await;
    }
}

/// `--take-over`: moves the coordinator here from the server it runs on.
async fn take_over_here(standby: &mut Standby) -> eyre::Result<()> {
    let record = standby.cloudflare.record(&standby.cfg.records[0]).await.map_err(|e| eyre::eyre!(e))?;
    match standby.where_is(record.content).await {
        Where::Here => Ok(println!("The coordinator already runs here.")),
        Where::Elsewhere(ip) => Err(eyre::eyre!("{} points at {ip}, none of the group's servers", record.name)),
        Where::There(i) => {
            standby.take_over(i, false).await.map_err(|e| eyre::eyre!(e))?;
            println!("The coordinator runs here now; {} stands down within a minute.", standby.cfg.servers[i].name);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::sync::Mutex;

    use super::*;

    const GROUP: &str = "
# Written by install-server.sh
record play.example.net
record metrics.example.net
me na1
address 198.51.100.2
server eu1 eu1.example.net
server na1 na1.example.net
server oceania oceania.example.net
";

    #[test]
    fn the_group_is_read_and_each_server_knows_its_wait() {
        let cfg = Config::parse(GROUP).unwrap();
        assert_eq!(cfg.records, ["play.example.net", "metrics.example.net"]);
        assert_eq!(cfg.probe, "https://play.example.net/v1/info");
        assert_eq!(cfg.database, PathBuf::from("/var/lib/5th-echelon-coordinator/coordinator.db"));
        assert_eq!(cfg.peer_scheme, "https");
        // eu1 runs it: na1 is first in line, oceania after.
        assert_eq!(cfg.wait(0), Duration::from_secs(180));
        let oceania = Config::parse(&GROUP.replace("me na1", "me oceania")).unwrap();
        assert_eq!(oceania.wait(0), Duration::from_secs(360));
        // na1 runs it (after a takeover): eu1 is first, then oceania.
        assert_eq!(oceania.wait(1), Duration::from_secs(360));
        let eu1 = Config::parse(&GROUP.replace("me na1", "me eu1")).unwrap();
        assert_eq!(eu1.wait(1), Duration::from_secs(180));
        for bad in [
            GROUP.replace("me na1", "me ap1"),
            GROUP.replace("address 198.51.100.2", ""),
            GROUP.replace("server oceania oceania.example.net", "server eu1 oceania.example.net"),
            GROUP.replace("record metrics.example.net", "record metrics example"),
            format!("{GROUP}peer-scheme ftp\n"),
            String::from("record play.example.net\nme na1\naddress 198.51.100.2\nserver na1 na1.example.net\n"),
        ] {
            assert!(Config::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_standby_waits_its_turn_and_a_replaced_coordinator_stands_down() {
        let cfg = Config::parse(GROUP).unwrap();
        let t0 = Instant::now();
        let down = Health::Down(String::from("521"));
        let mut w = Watch::default();
        assert_eq!(w.decide(&cfg, Some(&Where::There(0)), &Health::Up, false, t0), Action::Nothing);
        assert_eq!(w.decide(&cfg, Some(&Where::There(0)), &down, false, t0), Action::Waiting(Duration::from_secs(180)));
        // No answer from Cloudflare: the wait neither starts over nor ends.
        let unknown = Health::Unknown(String::from("timed out"));
        assert_eq!(
            w.decide(&cfg, Some(&Where::There(0)), &unknown, false, t0 + Duration::from_secs(60)),
            Action::Waiting(Duration::from_secs(120))
        );
        assert_eq!(
            w.decide(&cfg, Some(&Where::There(0)), &unknown, false, t0 + Duration::from_secs(200)),
            Action::Waiting(Duration::ZERO)
        );
        assert_eq!(w.decide(&cfg, Some(&Where::There(0)), &down, false, t0 + Duration::from_secs(200)), Action::Due(0));
        // Back up: the wait starts over.
        assert_eq!(w.decide(&cfg, Some(&Where::There(0)), &Health::Up, false, t0 + Duration::from_secs(210)), Action::Nothing);
        assert_eq!(
            w.decide(&cfg, Some(&Where::There(0)), &down, false, t0 + Duration::from_secs(220)),
            Action::Waiting(Duration::from_secs(180))
        );
        // A try that didn't work waits before the next.
        w.retry_after = Some(t0 + Duration::from_secs(700));
        assert_eq!(
            w.decide(&cfg, Some(&Where::There(0)), &down, false, t0 + Duration::from_secs(500)),
            Action::Waiting(Duration::from_secs(200))
        );
        // The record points here: the coordinator runs here.
        assert_eq!(w.decide(&cfg, Some(&Where::Here), &Health::Unknown(String::new()), false, t0), Action::Start);
        assert_eq!(w.decide(&cfg, Some(&Where::Here), &Health::Unknown(String::new()), true, t0), Action::Nothing);
        assert!(w.was_here);
        // Cloudflare's API doesn't answer: a coordinator that ran here starts (after a reboot).
        assert_eq!(w.decide(&cfg, None, &Health::Unknown(String::new()), false, t0), Action::Start);
        // It points at another server (one took over): this one stands down.
        assert_eq!(w.decide(&cfg, Some(&Where::There(2)), &Health::Up, true, t0), Action::StandDown);
        assert!(!w.was_here);
        assert_eq!(w.decide(&cfg, None, &Health::Unknown(String::new()), false, t0), Action::Nothing);
        // An address that's none of the group's: left alone.
        let mut w = Watch::default();
        assert_eq!(w.decide(&cfg, Some(&Where::Elsewhere("192.0.2.9".parse().unwrap())), &down, false, t0), Action::Nothing);
    }

    #[derive(Default)]
    struct Fake {
        running: AtomicBool,
        restored_from: Mutex<Vec<String>>,
    }

    impl Machine for Fake {
        fn running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }
        fn start(&self) -> Result<(), String> {
            self.running.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&self) -> Result<(), String> {
            self.running.store(false, Ordering::SeqCst);
            Ok(())
        }
        fn restore(&self, from: &str) -> Result<(), String> {
            self.restored_from.lock().unwrap().push(from.to_string());
            self.running.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Cloudflare's DNS API (records by name, changed by id), the coordinator through
    /// Cloudflare (`/v1/info`, answering with `status`), and a game server's `/api/info`.
    #[derive(Default)]
    struct Mock {
        records: Mutex<Vec<Value>>,
        status: std::sync::atomic::AtomicU16,
        seen: Mutex<Option<u64>>,
    }

    async fn serve(mock: Arc<Mock>) -> String {
        use axum::extract::Path as P;
        use axum::extract::Query;
        use axum::extract::State as S;
        use axum::http::HeaderMap;
        use axum::http::StatusCode;
        use axum::routing::get;
        use axum::Json;
        fn authorized(h: &HeaderMap) -> bool {
            h.get("authorization").is_some_and(|v| v == "Bearer token")
        }
        let app = axum::Router::new()
            .route(
                "/zones/zone/dns_records",
                get(|S(m): S<Arc<Mock>>, h: HeaderMap, Query(q): Query<HashMap<String, String>>| async move {
                    if !authorized(&h) {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(serde_json::json!({ "success": false, "errors": [{ "code": 9109, "message": "Invalid access token" }] })),
                        );
                    }
                    let list: Vec<Value> = m
                        .records
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|r| r["name"].as_str() == q.get("name").map(String::as_str))
                        .cloned()
                        .collect();
                    (StatusCode::OK, Json(serde_json::json!({ "success": true, "result": list })))
                }),
            )
            .route(
                "/zones/zone/dns_records/{id}",
                axum::routing::patch(|S(m): S<Arc<Mock>>, P(id): P<String>, Json(body): Json<Value>| async move {
                    for r in m.records.lock().unwrap().iter_mut() {
                        if r["id"] == id.as_str() {
                            r["content"] = body["content"].clone();
                        }
                    }
                    Json(serde_json::json!({ "success": true, "result": {} }))
                }),
            )
            .route(
                "/v1/info",
                get(|S(m): S<Arc<Mock>>| async move {
                    let status = StatusCode::from_u16(m.status.load(Ordering::SeqCst)).unwrap();
                    (status, Json(serde_json::json!({ "name": COORDINATOR_NAME })))
                }),
            )
            .route("/local/v1/info", get(|| async { Json(serde_json::json!({ "name": COORDINATOR_NAME })) }))
            .route(
                "/api/info",
                get(|S(m): S<Arc<Mock>>| async move {
                    let seen = *m.seen.lock().unwrap();
                    Json(serde_json::json!({ "name": "5th Echelon Enhanced", "coordinator_seen": seen }))
                }),
            )
            .with_state(mock);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn record(id: &str, name: &str, kind: &str, content: &str) -> Value {
        serde_json::json!({ "id": id, "name": name, "type": kind, "content": content, "proxied": true })
    }

    #[tokio::test]
    async fn a_standby_takes_over_from_a_coordinator_that_stays_down() {
        let mock = Arc::new(Mock::default());
        mock.status.store(200, Ordering::SeqCst);
        *mock.records.lock().unwrap() = vec![record("r1", "play.example.net", "A", "192.0.2.1"), record("r2", "metrics.example.net", "A", "192.0.2.1")];
        let url = serve(Arc::clone(&mock)).await;
        let dir = std::env::temp_dir().join(format!("standby-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // eu1 and na1 by address (no DNS in a test); this is na1.
        let text = format!(
            "record play.example.net\nrecord metrics.example.net\nme na1\naddress 192.0.2.2\nserver eu1 192.0.2.1\nserver na1 192.0.2.2\napi {url}\nprobe {url}/v1/info\nlocal {url}/local\nstate {}\ndatabase {}\n",
            dir.display(),
            dir.join("coordinator.db").display()
        );
        let cfg = Config::parse(&text).unwrap();
        let http = reqwest::Client::new();
        let fake = Arc::new(Fake::default());
        let mut standby = Standby::new(
            cfg,
            Cloudflare::new(http.clone(), &url, "zone", "token"),
            Arc::clone(&fake) as Arc<dyn Machine>,
            http.clone(),
        );

        // eu1 answers: na1 stands by.
        assert_eq!(standby.tick().await, Action::Nothing);
        // It doesn't: na1 waits its turn, then takes over.
        mock.status.store(521, Ordering::SeqCst);
        assert_eq!(standby.tick().await, Action::Waiting(WAIT));
        standby.watch.down_since = Some(Instant::now() - WAIT - Duration::from_secs(1));
        assert_eq!(standby.tick().await, Action::Due(0));
        assert_eq!(*fake.restored_from.lock().unwrap(), ["eu1"]);
        assert!(fake.running());
        let contents: Vec<Value> = mock.records.lock().unwrap().iter().map(|r| r["content"].clone()).collect();
        assert_eq!(contents, ["192.0.2.2", "192.0.2.2"], "both records point at na1");
        assert_eq!(std::fs::read_to_string(dir.join("primary")).unwrap().trim(), "na1");
        // The coordinator runs here now (its database restored).
        std::fs::write(dir.join("coordinator.db"), "").unwrap();
        mock.status.store(200, Ordering::SeqCst);
        assert_eq!(standby.tick().await, Action::Nothing);
        fake.stop().unwrap();
        assert_eq!(standby.tick().await, Action::Start);
        // Someone moves it back to eu1: na1 stands down.
        mock.records.lock().unwrap()[0]["content"] = Value::from("192.0.2.1");
        assert_eq!(standby.tick().await, Action::StandDown);
        assert!(!fake.running());

        // A record with an AAAA beside it isn't one failover can move.
        mock.records.lock().unwrap().push(record("r3", "play.example.net", "AAAA", "2001:db8::1"));
        let e = standby.cloudflare.record("play.example.net").await.unwrap_err();
        assert!(e.contains("exactly one A record"), "{e}");
        // A wrong token is Cloudflare's error, said.
        let wrong = Cloudflare::new(http.clone(), &url, "zone", "nope");
        assert!(wrong.record("metrics.example.net").await.unwrap_err().contains("Invalid access token"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn health_through_cloudflare_and_whether_peers_reach_the_coordinator() {
        let mock = Arc::new(Mock::default());
        let url = serve(Arc::clone(&mock)).await;
        let http = reqwest::Client::new();
        let probe_url = format!("{url}/v1/info");
        for (status, health) in [(200, "Up"), (502, "Down"), (522, "Down"), (403, "Unknown")] {
            mock.status.store(status, Ordering::SeqCst);
            assert!(format!("{:?}", probe(&http, &probe_url).await).starts_with(health), "{status}");
        }
        // Nothing listening: no answer, not an error from the coordinator's server.
        assert!(matches!(probe(&http, "http://127.0.0.1:9/v1/info").await, Health::Unknown(_)));
        let host = url.trim_start_matches("http://");
        assert_eq!(reaches(&http, "http", host).await, Some(false), "no heartbeat yet");
        *mock.seen.lock().unwrap() = Some(20);
        assert_eq!(reaches(&http, "http", host).await, Some(true));
        *mock.seen.lock().unwrap() = Some(400);
        assert_eq!(reaches(&http, "http", host).await, Some(false));
        assert_eq!(reaches(&http, "http", "127.0.0.1:9").await, None);
    }

    #[test]
    fn the_secrets_file_is_only_its_owners() {
        let path = std::env::temp_dir().join(format!("standby-secrets-{}", std::process::id()));
        std::fs::write(&path, "# Cloudflare\nCLOUDFLARE_API_TOKEN=abc\nCLOUDFLARE_ZONE_ID=\"z1\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(secrets(&path).unwrap_err().contains("mode 600"));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(secrets(&path).unwrap(), (String::from("abc"), String::from("z1")));
        std::fs::write(&path, "CLOUDFLARE_API_TOKEN=abc\n").unwrap();
        assert!(secrets(&path).unwrap_err().contains("CLOUDFLARE_ZONE_ID"));
        std::fs::remove_file(path).unwrap();
    }
}
