//! The Roadmap screen: what's being built, from the community coordinator's roadmap
//! (whatever network the player is on), kept on disk so it shows offline; a form to suggest
//! a feature, signed with the player's identity; and the player's own suggestions with the
//! admins' replies.

use std::time::Duration;

use setup::roadmap::Mine;

/// How long the project's server may take to answer.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A suggestion as sent, to send again.
#[derive(Debug, Clone)]
pub struct Draft {
    pub area: String,
    pub title: String,
    pub text: String,
    pub name: String,
    pub server: Option<String>,
}

/// The project's server: a client that waits 10 s at most and follows no redirects.
pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

/// The answer's body, as text, up to [`setup::roadmap::MAX_BYTES`]; or the server's own words
/// when it said no.
pub async fn body(resp: reqwest::Response) -> Result<String, String> {
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

pub fn unreachable(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "The project's server didn't answer in time.".into()
    } else {
        "The project's server didn't answer. Check your connection and try again.".into()
    }
}

/// Reads the roadmap, and keeps it for when the server can't be reached.
pub fn fetch_roadmap() -> Result<setup::roadmap::Roadmap, String> {
    let roadmap = crate::services::rt().block_on(async {
        let resp = client()?
            .get(setup::roadmap::url(setup::directory::community()))
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
pub fn load_identity() -> Result<identity::Identity, String> {
    match setup::player_identity::load() {
        Ok(Some(identity)) => Ok(identity),
        Ok(None) => Err("Connect to a server first: your identity, which signs suggestions, is made then.".into()),
        Err(e) => Err(format!("Your identity can't be read here: {e}")),
    }
}

/// Whether this PC has an identity yet (made when the player first connects).
pub fn has_identity() -> bool {
    setup::player_identity::path().is_some_and(|p| p.exists())
}

pub fn send(draft: &Draft) -> Result<(), String> {
    let identity = load_identity()?;
    let sender = setup::roadmap::Sender {
        name: &draft.name,
        server: draft.server.as_deref(),
        launcher: env!("FE_RELEASE"),
    };
    let json = setup::roadmap::suggestion_body(&identity, &sender, &draft.area, &draft.title, &draft.text, identity::now());
    crate::services::rt().block_on(async {
        let resp = client()?
            .post(format!("{}/v1/suggestions", setup::directory::community()))
            .header("content-type", "application/json")
            .body(json.to_string())
            .send()
            .await
            .map_err(unreachable)?;
        body(resp).await.map(|_| ())
    })
}

pub fn fetch_mine() -> Result<Vec<Mine>, String> {
    let identity = load_identity()?;
    let url = setup::roadmap::mine_url(setup::directory::community(), &identity, identity::now());
    crate::services::rt().block_on(async {
        let resp = client()?.get(url).header("accept", "application/json").send().await.map_err(unreachable)?;
        let body = body(resp).await?;
        setup::roadmap::parse_mine(&body).map_err(|_| "The project's server sent an answer the launcher can't read.".to_string())
    })
}
