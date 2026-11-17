//! The launcher's connection test (Settings › Network) without the window, for test rigs where
//! the launcher itself can't run: `connection_test <server host> [api url]`. Prints each part's
//! line as the launcher shows it. The account: FE_TEST_USER and FE_TEST_PASSWORD. Used on the
//! 0.5.5 router rig (a PC behind a router where another PC holds the game's port).
fn main() {
    let mut args = std::env::args().skip(1);
    let server = args.next().expect("connection_test <server host> [api url]");
    let api = args.next().unwrap_or_else(|| format!("https://{server}/"));
    let mut profile = setup::config::Profile {
        name: server.clone(),
        server,
        api_server_url: Some(api.parse().expect("an API URL")),
        ..Default::default()
    };
    if let (Ok(user), Ok(password)) = (std::env::var("FE_TEST_USER"), std::env::var("FE_TEST_PASSWORD")) {
        profile.user.username = user;
        profile.user.set_secret(&password);
    }
    let dir = std::env::temp_dir();
    for (name, status, note) in launcher::settings::run_tests(&profile, None, hooks_config::NatMode::Auto, &dir) {
        println!("{status:?}\t{name}\t{}", note.unwrap_or_default());
    }
}
