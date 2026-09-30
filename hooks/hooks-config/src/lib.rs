use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;
use serde::Serialize;
use tracing::info;
use tracing::instrument;
use url::Url;

#[cfg(target_os = "windows")]
mod saves;
#[cfg(target_os = "windows")]
pub use saves::SaveGameExt;

#[cfg(target_os = "windows")]
mod msgbox {
    use std::ffi::CString;

    use windows::core::PCSTR;
    use windows::Win32::UI::WindowsAndMessaging::MessageBoxA;
    use windows::Win32::UI::WindowsAndMessaging::IDOK;
    use windows::Win32::UI::WindowsAndMessaging::MB_ICONQUESTION;
    use windows::Win32::UI::WindowsAndMessaging::MB_OK;
    use windows::Win32::UI::WindowsAndMessaging::MB_OKCANCEL;

    pub fn show_msgbox(msg: &str, caption: &str) {
        let msg = CString::new(msg).unwrap();
        let caption = CString::new(caption).unwrap();
        unsafe {
            MessageBoxA(None, PCSTR(msg.as_ptr().cast::<u8>()), PCSTR(caption.as_ptr().cast::<u8>()), MB_OK);
        }
    }

    pub fn show_msgbox_ok_cancel(msg: &str, caption: &str) -> bool {
        let msg = CString::new(msg).unwrap();
        let caption = CString::new(caption).unwrap();
        unsafe { MessageBoxA(None, PCSTR(msg.as_ptr().cast::<u8>()), PCSTR(caption.as_ptr().cast::<u8>()), MB_OKCANCEL | MB_ICONQUESTION) == IDOK }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();
pub static URL: OnceLock<Url> = OnceLock::new();

fn default_password() -> String {
    String::from("password1234")
}
fn default_username() -> String {
    String::from("sam_the_fisher")
}
fn default_account_id() -> String {
    String::from("00000000-0000-4000-0000-000000000000")
}
fn default_overlay() -> bool {
    true
}

macro_rules! enum_gui {
    (
        $(#[$attr:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[cfg(feature = $feature:literal)])?
                #[label=$label:literal]
                $field:ident,
            )*
        }
    ) => {
        $(#[$attr])*
        $vis enum $name {
            $(
                $(#[cfg(feature = $feature)])?
                $field,
            )*
        }

        impl $name {
            enum_gui!(@1 $name, [$($(#[cfg(feature = $feature)])?$name::$field),*]);
            enum_gui!(@2 [$($(#[cfg(feature = $feature)])?$label),*]);
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(
                        $(#[cfg(feature = $feature)])?
                        $name::$field => write!(f, "{}::{}", stringify!($name), stringify!($field)),
                    )*
                }
            }
        }
    };

    (@1 $name:ident, $value:expr) => {
        pub const VARIANTS: [$name; $value.len()] = $value;
    };

    (@2 $value:expr) => {
        pub const LABELS: [&'static str; $value.len()] = $value;
    };
}

enum_gui! {
    #[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
    #[serde(rename_all = "PascalCase")]
    pub enum Hook {
        #[label="Log internal messages"]
        Printer,
        #[label="Log when a state is exiting"]
        LeaveState,
        #[label="Log what state is coming up next"]
        NextState,
        #[label="Log NetResultBase"]
        NetResultBase,
        #[label="Log goals"]
        Goal,
        #[label="Log next step"]
        SetStep,
        #[label="Log new threads"]
        Thread,
        #[label="Log state changes"]
        ChangeState,
        #[label="Enforce LAN mode?"]
        NetCore,
        #[label="Log NetResultCore"]
        NetResultCore,
        #[label="Log NetResultSession"]
        NetResultSession,
        #[label="Log NetResultRdvSession"]
        NetResultRdvSession,
        #[label="Log NetResultLobby"]
        NetResultLobby,
        #[label="Log IP:PORT used by Storm"]
        StormHostPortToString,
        #[label="Enforce IP returned by GetAdaptersInfo"]
        GetAdaptersInfo,
        #[label="Enforce IP returned by gethostbyname"]
        Gethostbyname,
        #[label="Log generated IDs"]
        GenerateID,
        #[label="Log storm states"]
        StormSetState,
        #[label="Log storm state transitions"]
        StormStateMachineActionExecute,
        #[label="Log storm errors"]
        StormErrorFormatter,
        #[label="Log gear destructors (LARGE)"]
        GearStrDestructor,
        #[label="Log storm events"]
        StormEventDispatcher,
        #[label="Log storm udp packets"]
        StormPackets,
        #[label="Log RMC messages"]
        RMCMessages,
        #[cfg(feature = "modding")]
        #[label="Override packaged files"]
        OverridePackaged,
    }
}

/// Which field of Uplay's friend structure carries the session a friend can be joined in.
///
/// Blacklist reads a friend's session straight out of the structure that
/// `UPLAY_FRIENDS_GetFriendList` fills in - it does not ask a second time. Either it finds
/// the session there, or an accepted invitation dies with "Failed to find party session for
/// invite." Which of the unnamed fields holds it has not been established, so the candidates
/// are selectable here: trying one out costs a game restart instead of a rebuild.
///
/// The names follow the fields of the `Friend` and `FriendDetails` structures in
/// `hooks/src/uplay_r1_loader/types.rs`.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum SessionField {
    /// Fill nothing in - how it behaved before this existed.
    #[default]
    None,
    /// `Friend.unknown1`
    FriendUnknown1,
    /// `Friend.unknown2`
    FriendUnknown2,
    /// `Friend.unknown3`
    FriendUnknown3,
    /// `FriendDetails.unknown3`
    DetailsUnknown3,
    /// `FriendDetails.unknown2`, as a decimal string
    DetailsUnknown2Str,
}

/// Which Uplay event an accepted invitation raises.
///
/// The kind of event decides which join route the game takes afterwards:
///
/// * `Friends` (`FriendsGameInviteAccepted`, event 10002) makes the client search with
///   `SearchSessionsWithParticipants` (protocol 42, method 24) and then run
///   `AbandonSession` -> `SplitSession` -> `AddParticipants`.
/// * `Party` (`PartyGameInviteAccepted`, event 20004) goes through the party layer instead.
///
/// The client picks its route on its own; no server answer redirects it. That is why this
/// switch lives here and not in the server.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum InviteAcceptEvent {
    /// `FriendsGameInviteAccepted` - the previous behaviour.
    #[default]
    Friends,
    /// `PartyGameInviteAccepted`.
    Party,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    pub user: User,
    #[serde(default)]
    #[serde(skip_serializing_if = "Save::is_default")]
    pub save: Save,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forward_calls: Vec<String>,
    #[serde(default)]
    pub forward_all_calls: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub internal_command_line: String,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub enable_hooks: HashSet<Hook>,
    #[serde(default)]
    pub enable_all_hooks: bool,
    #[serde(default = "default_overlay")]
    pub enable_overlay: bool,
    /// Shows the overlay's developer window (send test invite events). Off for
    /// players.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overlay_debug: bool,
    pub config_server: Option<String>,
    pub api_server: url::Url,
    #[serde(default)]
    #[serde(skip_serializing_if = "Networking::is_default")]
    pub networking: Networking,
    #[serde(default)]
    #[serde(skip_serializing_if = "Logging::is_default")]
    pub logging: Logging,
    #[serde(default)]
    pub auto_join_invite: bool,
    /// Let the server join you to a match without your click (its `force_join`).
    /// Off: a server shouldn't be able to put you into a match by itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_force_join: bool,
    /// Which Uplay event an accepted invitation raises. See [`InviteAcceptEvent`]; switching
    /// costs a game restart rather than a rebuild.
    #[serde(default)]
    pub invite_accept_event: InviteAcceptEvent,
    /// Where a friend's session is written into the Uplay friend structure. See
    /// [`SessionField`]; only needed while the right field is still being determined.
    #[serde(default)]
    pub session_field: SessionField,
    /// Whether a friend's session payload is passed on in `FriendDetails.unknown4`.
    ///
    /// Without it the other side gets past the lookup but opens a session of its own instead
    /// of joining. Switchable so that a counter-test does not need a rebuild.
    #[serde(default = "default_share_session_data")]
    pub share_session_data: bool,

    /// Where a friend's principal id is written into the Uplay friend structure. See
    /// [`PidField`].
    #[serde(default)]
    pub pid_field: PidField,

    /// Set when another tool manages this install (from the override file's
    /// `[Managed]`); never read from or written to `uplay.toml`.
    #[serde(skip)]
    pub managed: Option<Managed>,
}

const fn default_share_session_data() -> bool {
    true
}

/// Which field of Uplay's friend structure carries a friend's Quazal principal id.
///
/// On the working matchmaking path the guest finds the host through
/// `SearchSessionsWithParticipants`, which takes participant ids. On the invitation path the
/// game never makes that call - the suspicion being that it has no pid to search with,
/// because the friend structure only ever held the ubi id as a string.
///
/// `Friend.unknown1` is taken by the session (see [`SessionField`]), so the remaining
/// candidates are the ones below.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum PidField {
    /// Fill nothing in.
    #[default]
    None,
    /// `Friend.unknown2`
    FriendUnknown2,
    /// `Friend.unknown3`
    FriendUnknown3,
    /// `FriendDetails.unknown3`
    DetailsUnknown3,
}

#[derive(Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    #[serde(default = "default_username")]
    pub username: String,
    /// The password in plain text. Empty when it's kept in
    /// `ProtectedPassword` instead.
    #[serde(default = "default_password")]
    pub password: String,
    /// The password, encrypted for this Windows user (DPAPI, hex). Written
    /// by the launcher on Windows; only this Windows account on this PC can
    /// read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub protected_password: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cd_keys: Vec<String>,
    #[serde(default = "default_account_id")]
    pub account_id: String,
}

/// Never prints the password: configs end up in logs and error messages.
impl std::fmt::Debug for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hidden = |s: &str| if s.is_empty() { "" } else { "<hidden>" };
        f.debug_struct("User")
            .field("username", &self.username)
            .field("password", &hidden(&self.password))
            .field("protected_password", &hidden(&self.protected_password))
            .field("cd_keys", &self.cd_keys.len())
            .field("account_id", &self.account_id)
            .finish()
    }
}

impl User {
    /// The password to sign in with: decrypted from `ProtectedPassword`, or
    /// `Password`. None when the protected one can't be read here (another
    /// PC or Windows account).
    pub fn secret(&self) -> Option<String> {
        if self.protected_password.is_empty() {
            Some(self.password.clone())
        } else {
            protect::unprotect(&self.protected_password)
        }
    }

    /// Stores `password`, encrypted where this system can (DPAPI on Windows,
    /// but not under Wine, whose prefixes don't share keys); in plain text
    /// otherwise.
    pub fn set_secret(&mut self, password: &str) {
        match protect::protect(password) {
            Some(blob) => {
                self.protected_password = blob;
                self.password = String::new();
            }
            None => {
                self.protected_password = String::new();
                self.password = password.to_string();
            }
        }
    }
}

/// Encrypting the saved password for the Windows user (DPAPI).
pub mod protect {
    /// `secret` encrypted and hex-encoded; None where DPAPI isn't usable.
    pub fn protect(secret: &str) -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            if crate::running_under_wine() {
                return None;
            }
            imp::run(secret.as_bytes(), true).map(|b| hex(&b))
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = secret;
            None
        }
    }

    /// Decrypts [`protect`]'s output; None if this user can't.
    pub fn unprotect(blob: &str) -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            let bytes = unhex(blob)?;
            imp::run(&bytes, false).and_then(|b| String::from_utf8(b).ok())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = blob;
            None
        }
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn unhex(text: &str) -> Option<Vec<u8>> {
        let text = text.trim();
        if text.len() % 2 != 0 {
            return None;
        }
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
    }

    #[cfg(target_os = "windows")]
    mod imp {
        use windows::Win32::Foundation::LocalFree;
        use windows::Win32::Foundation::HLOCAL;
        use windows::Win32::Security::Cryptography::CryptProtectData;
        use windows::Win32::Security::Cryptography::CryptUnprotectData;
        use windows::Win32::Security::Cryptography::CRYPTPROTECT_UI_FORBIDDEN;
        use windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB;

        /// Encrypts (`protect`) or decrypts `data` for the current user.
        pub fn run(data: &[u8], protect: bool) -> Option<Vec<u8>> {
            let input = CRYPT_INTEGER_BLOB {
                cbData: u32::try_from(data.len()).ok()?,
                pbData: data.as_ptr().cast_mut(),
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            // SAFETY: the input blob points at `data`, which outlives the call; the output is
            // allocated by Windows and freed with LocalFree below.
            unsafe {
                let done = if protect {
                    CryptProtectData(&input, windows::core::PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
                } else {
                    CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
                };
                done.ok()?;
                let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
                let _ = LocalFree(HLOCAL(output.pbData.cast()));
                Some(bytes)
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn hex_round_trips() {
            let bytes = [0u8, 1, 0xab, 0xff];
            assert_eq!(super::unhex(&super::hex(&bytes)).unwrap(), bytes);
            assert!(super::unhex("abc").is_none());
            assert!(super::unhex("zz").is_none());
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum SaveDir {
    InstallLocation,
    #[default]
    Roaming,
    Custom(String),
}

impl SaveDir {
    pub fn is_default(&self) -> bool {
        matches!(self, SaveDir::Roaming)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Save {
    #[serde(default)]
    #[serde(skip_serializing_if = "SaveDir::is_default")]
    pub save_dir: SaveDir,
}

impl Save {
    pub fn is_default(&self) -> bool {
        self.save_dir.is_default()
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct SaveGame {
    pub slot_id: usize,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Networking {
    pub ip_address: Option<std::net::Ipv4Addr>,
    pub adapter: Option<String>,
    /// Refuse to start unless `adapter` exists and has an address, instead of
    /// quietly advertising another network (the home LAN, Radmin, ...) to
    /// other players, which makes their joins fail.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_adapter: bool,
    /// Playing over the internet without a VPN; see [`NatMode`].
    #[serde(default, skip_serializing_if = "NatMode::is_default")]
    pub nat: NatMode,
    /// Ask the router (UPnP, then NAT-PMP) to forward the game's
    /// peer-to-peer port while the game runs.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub port_mapping: bool,
    /// The server's NAT helper port (UDP, 21128 unless the server changed it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nat_port: Option<u16>,
}

const fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(b: &bool) -> bool {
    *b
}

impl Default for Networking {
    fn default() -> Self {
        Self {
            ip_address: None,
            adapter: None,
            require_adapter: false,
            nat: NatMode::default(),
            port_mapping: true,
            nat_port: None,
        }
    }
}

impl Networking {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// How the game gets through the players' routers (NAT) to reach the others.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum NatMode {
    /// Ask the server's NAT helper for this PC's public address and advertise
    /// it, so players can connect directly; go through the server's relay
    /// when the router can't be punched through.
    #[default]
    Auto,
    /// Always go through the server's relay. For routers direct connections
    /// don't work with; adds a little latency.
    Relay,
    /// Advertise the local address only (the game's own behaviour): for LAN
    /// and VPN play.
    Off,
}

impl NatMode {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Does the Windows adapter `friendly_name` match the configured `wanted`?
///
/// Case-insensitive, and tolerant of the "<name> - <ip>" form that upstream's
/// launcher saved from its adapter picker, which never matched any adapter
/// (so the pin silently did nothing).
pub fn adapter_name_matches(friendly_name: &str, wanted: &str) -> bool {
    let wanted = match wanted.rsplit_once(" - ") {
        Some((name, ip)) if ip.trim().parse::<std::net::IpAddr>().is_ok() => name,
        _ => wanted,
    };
    friendly_name.trim().eq_ignore_ascii_case(wanted.trim())
}

/// Settings managed by another tool (an installer, a VPN client, a
/// community's own app), read from `uplay.override.toml` next to
/// `uplay.toml`. They take precedence over `uplay.toml`, which the launcher
/// rewrites whenever it launches the game; the launcher never touches this
/// file.
///
/// With this file present the game also starts without the launcher ever
/// having run (see `_get_or_load`), so a tool can install and set up the
/// game on its own.
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Overrides {
    pub config_server: Option<String>,
    pub api_server: Option<url::Url>,
    pub user: Option<User>,
    pub networking: Option<Networking>,
    pub managed: Option<Managed>,
}

/// The tool that manages this install. The launcher shows its settings as
/// read-only and leaves updates to that tool; the overlay points players to
/// it when something needs fixing.
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Managed {
    /// The tool's name, e.g. "Community Hub".
    pub by: String,
    /// What to tell players when the server can't be reached or the account
    /// is refused, e.g. "Open Community Hub and check the server is online."
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help_text: Option<String>,
}

impl Overrides {
    /// Applies the overrides to `cfg`.
    pub fn apply(self, cfg: &mut Config) {
        if let Some(server) = self.config_server {
            cfg.config_server = Some(server);
        }
        if let Some(api) = self.api_server {
            cfg.api_server = api;
        }
        if let Some(user) = self.user {
            // Keep CD keys from uplay.toml unless the overrides set their own.
            let cd_keys = if user.cd_keys.is_empty() {
                std::mem::take(&mut cfg.user.cd_keys)
            } else {
                user.cd_keys.clone()
            };
            cfg.user = User { cd_keys, ..user };
        }
        if let Some(networking) = self.networking {
            cfg.networking = networking;
        }
        if let Some(managed) = self.managed {
            cfg.managed = Some(managed);
        }
    }
}

/// File name of the overrides, next to `uplay.toml`.
pub const OVERRIDES_FILE: &str = "uplay.override.toml";

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, PartialOrd, Eq, Ord, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warning,
    Error,
}

impl AsRef<str> for LogLevel {
    fn as_ref(&self) -> &str {
        match self {
            LogLevel::Trace => "Trace",
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Warning => "Warning",
            LogLevel::Error => "Error",
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct Logging {
    #[serde(default)]
    pub level: LogLevel,
}

impl Logging {
    pub fn is_default(&self) -> bool {
        matches!(self.level, LogLevel::Info)
    }
}

const DEFAULT_CONFIG: &str = r#"
# Where to find the config server
ConfigServer = "127.0.0.1"
# Where to find the api server (typically the same as the config server)
ApiServer = "http://127.0.0.1:50051"
# Automatically join invites without user intervention
AutoJoinInvite = false
# Which Uplay event an accepted invitation raises: "Friends" or "Party".
# It decides which join route the game takes - see InviteAcceptEvent.
InviteAcceptEvent = "Friends"

[User]
# Username for the community server
Username = "sam_the_fisher"
# Password for the community server
Password = "password1234"

[Save]
SaveDir = "Roaming"
"#;

pub fn get() -> Option<&'static Config> {
    CONFIG.get()
}

#[cfg(target_os = "windows")]
fn get_adapter_infos<T>(f: impl Fn(*mut windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_INFO) -> anyhow::Result<T>) -> anyhow::Result<T> {
    #![allow(clippy::cast_possible_truncation, clippy::crosspointer_transmute)]

    use anyhow::bail;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::Foundation::HLOCAL;
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::NetworkManagement::IpHelper::GetAdaptersInfo;
    use windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_INFO;
    use windows::Win32::System::Diagnostics::Debug::FormatMessageW;
    use windows::Win32::System::Diagnostics::Debug::FORMAT_MESSAGE_ALLOCATE_BUFFER;

    let mut adapterinfo = vec![0u8; std::mem::size_of::<IP_ADAPTER_INFO>()];
    let mut size = adapterinfo.len() as u32;
    let mut res = WIN32_ERROR(unsafe { GetAdaptersInfo(Some(adapterinfo.as_mut_ptr().cast()), &mut size) });
    if res == ERROR_BUFFER_OVERFLOW {
        adapterinfo.resize(size as usize, 0);
        res = WIN32_ERROR(unsafe { GetAdaptersInfo(Some(adapterinfo.as_mut_ptr().cast()), &mut size) });
    }
    match res {
        ERROR_BUFFER_OVERFLOW => {
            bail!("Couldn't allocate enough memory for network adapters");
        }
        ERROR_SUCCESS => f(adapterinfo.as_mut_ptr().cast()),
        _ => unsafe {
            let mut buffer_ptr: PWSTR = PWSTR::null();
            let ptr_ptr: *mut PWSTR = &mut buffer_ptr;
            let chars = FormatMessageW(FORMAT_MESSAGE_ALLOCATE_BUFFER, None, res.0, 0, std::mem::transmute::<*mut PWSTR, PWSTR>(ptr_ptr), 256, None);
            let msg = if chars > 0 {
                String::from_utf16(std::slice::from_raw_parts(buffer_ptr.0, chars as _))?
            } else {
                String::from("unknown")
            };
            LocalFree(std::mem::transmute::<PWSTR, HLOCAL>(buffer_ptr))?;
            bail!("Couldn't enumerate adapters: {}", msg);
        },
    }
}

#[cfg(target_os = "windows")]
fn get_adapter_addresses<T>(f: impl Fn(*mut windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH) -> anyhow::Result<T>) -> anyhow::Result<T> {
    #![allow(clippy::cast_possible_truncation, clippy::crosspointer_transmute, clippy::cast_lossless)]

    use anyhow::bail;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::Foundation::HLOCAL;
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::NetworkManagement::IpHelper::GetAdaptersAddresses;
    use windows::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_ANYCAST;
    use windows::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_DNS_SERVER;
    use windows::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_MULTICAST;
    use windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;
    use windows::Win32::Networking::WinSock::AF_INET;
    use windows::Win32::System::Diagnostics::Debug::FormatMessageW;
    use windows::Win32::System::Diagnostics::Debug::FORMAT_MESSAGE_ALLOCATE_BUFFER;

    let mut adapter_addresses = vec![0u8; std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>()];
    let mut size = adapter_addresses.len() as u32;
    let mut res = WIN32_ERROR(unsafe {
        GetAdaptersAddresses(
            AF_INET.0 as u32,
            GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
            None,
            Some(adapter_addresses.as_mut_ptr().cast()),
            &mut size,
        )
    });
    if res == ERROR_BUFFER_OVERFLOW {
        adapter_addresses.resize(size as usize, 0);
        res = WIN32_ERROR(unsafe {
            GetAdaptersAddresses(
                AF_INET.0 as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                None,
                Some(adapter_addresses.as_mut_ptr().cast()),
                &mut size,
            )
        });
    }
    match res {
        ERROR_BUFFER_OVERFLOW => {
            bail!("Couldn't allocate enough memory for network adapters");
        }
        ERROR_SUCCESS => f(adapter_addresses.as_mut_ptr().cast()),
        _ => unsafe {
            let mut buffer_ptr: PWSTR = PWSTR::null();
            let ptr_ptr: *mut PWSTR = &mut buffer_ptr;
            let chars = FormatMessageW(FORMAT_MESSAGE_ALLOCATE_BUFFER, None, res.0, 0, std::mem::transmute::<*mut PWSTR, PWSTR>(ptr_ptr), 256, None);
            let msg = if chars > 0 {
                String::from_utf16(std::slice::from_raw_parts(buffer_ptr.0, chars as _))?
            } else {
                String::from("unknown")
            };
            LocalFree(std::mem::transmute::<PWSTR, HLOCAL>(buffer_ptr))?;
            bail!("Couldn't enumerate adapters: {}", msg);
        },
    }
}

#[cfg(target_os = "windows")]
fn find_ipaddress_for_adapter(target_adapter: &str) -> anyhow::Result<Option<std::net::Ipv4Addr>> {
    use std::ffi::CStr;
    use std::net::Ipv4Addr;

    use tracing::debug;
    use tracing::warn;
    use windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;
    use windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_INFO;
    use windows::Win32::Networking::WinSock::AF_INET;
    use windows::Win32::Networking::WinSock::SOCKADDR_IN;

    if cfg!(feature = "GetAdapterInfos") {
        get_adapter_infos(|adapterinfo| {
            let mut result = None;
            let mut next_adapter: *mut IP_ADAPTER_INFO = adapterinfo;
            while let Some(current_adapter) = unsafe { next_adapter.as_ref() } {
                let adapter_name = CStr::from_bytes_until_nul(&current_adapter.Description)?;
                debug!("{adapter_name:?} == {target_adapter:?}");
                if adapter_name_matches(adapter_name.to_str()?, target_adapter) {
                    result = Some(CStr::from_bytes_until_nul(&current_adapter.IpAddressList.IpAddress.String)?.to_str()?.parse()?);
                    break;
                }
                next_adapter = current_adapter.Next;
            }
            if result.is_none() {
                warn!("No adapter {target_adapter:?} found");
            }
            Ok(result)
        })
    } else {
        get_adapter_addresses(|adapter_addresses| {
            let mut result = None;
            let mut next_adapter: *mut IP_ADAPTER_ADDRESSES_LH = adapter_addresses;
            while let Some(current_adapter) = unsafe { next_adapter.as_ref() } {
                let adapter_name = unsafe { current_adapter.FriendlyName.to_string()? };
                debug!("{adapter_name:?} == {target_adapter:?}");
                if adapter_name_matches(&adapter_name, target_adapter) {
                    if let Some(ip) = unsafe { current_adapter.FirstUnicastAddress.as_ref() } {
                        #[allow(clippy::cast_ptr_alignment)]
                        let sockaddr = unsafe { ip.Address.lpSockaddr.cast::<SOCKADDR_IN>().as_ref().unwrap() };
                        if sockaddr.sin_family == AF_INET {
                            result = Some(Ipv4Addr::new(
                                unsafe { sockaddr.sin_addr.S_un.S_un_b.s_b1 },
                                unsafe { sockaddr.sin_addr.S_un.S_un_b.s_b2 },
                                unsafe { sockaddr.sin_addr.S_un.S_un_b.s_b3 },
                                unsafe { sockaddr.sin_addr.S_un.S_un_b.s_b4 },
                            ));
                            break;
                        }
                        warn!("Can't handle family {:?}", sockaddr.sin_family);
                    }
                }
                next_adapter = current_adapter.Next;
            }
            if result.is_none() {
                warn!("No adapter {target_adapter:?} found");
            }
            Ok(result)
        })
    }
}

/// Whether the game runs under Wine or Proton (ntdll exports
/// `wine_get_version` there, never on Windows).
pub fn running_under_wine() -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::core::s;
        use windows::Win32::System::LibraryLoader::GetModuleHandleA;
        use windows::Win32::System::LibraryLoader::GetProcAddress;
        GetModuleHandleA(s!("ntdll.dll")).is_ok_and(|ntdll| GetProcAddress(ntdll, s!("wine_get_version")).is_some())
    }
    #[cfg(not(target_os = "windows"))]
    false
}

pub fn get_or_load(path: impl AsRef<Path>) -> anyhow::Result<&'static Config> {
    _get_or_load(path.as_ref())
}

#[instrument]
fn _get_or_load(path: &Path) -> anyhow::Result<&'static Config> {
    if let Some(cfg) = get() {
        return Ok(cfg);
    }
    let overrides_path = path.with_file_name(OVERRIDES_FILE);
    let overrides: Option<Overrides> = match fs::read_to_string(&overrides_path) {
        Ok(content) => Some(toml::from_str(&content).map_err(|e| anyhow::anyhow!("{}: {e}", overrides_path.display()))?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        // Set up by another tool; the launcher never ran.
        Err(ref err) if err.kind() == std::io::ErrorKind::NotFound && overrides.is_some() => DEFAULT_CONFIG.to_string(),
        #[cfg(target_os = "windows")]
        Err(ref err) if err.kind() == std::io::ErrorKind::NotFound && msgbox::show_msgbox_ok_cancel("Configuration not found, generate and exit?", "Configuration not found") => {
            fs::write(path, DEFAULT_CONFIG)?;
            msgbox::show_msgbox(&format!("Config file placed at {}", path.to_str().unwrap()), "Done");
            std::process::exit(0);
        }
        Err(err) => return Err(err.into()),
    };
    let mut cfg: Config = toml::from_str(&content)?;
    if let Some(overrides) = overrides {
        info!("Applying {}", overrides_path.display());
        overrides.apply(&mut cfg);
    }
    if cfg.user.cd_keys.is_empty() {
        // Without a CD key, startup goes to Ubisoft's own DLL, which starts
        // Ubisoft Connect. Under Wine/Proton there usually is none, and the
        // game hangs waiting for it; run without it there.
        if running_under_wine() {
            info!("Wine detected: running without Ubisoft Connect");
        } else {
            info!("Passing startup to original dll");
            cfg.forward_calls.push("UPLAY_Startup".into());
            cfg.forward_calls.push("UPLAY_Quit".into());
        }
        //        cfg.forward_calls.push("UPLAY_USER_GetCdKeys".into());
        cfg.user.cd_keys.push("ABCD-EFGH-IJKL-MNOP".into());
    }

    // A pinned adapter wins over a stored IpAddress, which goes stale (upstream
    // preferred the IP and never looked at the adapter when one was set). The
    // IP stays the fallback when the adapter isn't there.
    #[cfg(target_os = "windows")]
    if let Some(target_adapter) = cfg.networking.adapter.clone() {
        info!("Getting IP address of adapter {target_adapter:?}");
        match find_ipaddress_for_adapter(&target_adapter) {
            Ok(Some(ip)) => {
                if cfg.networking.ip_address.is_some_and(|stored| stored != ip) {
                    tracing::warn!("IpAddress {:?} is stale; using {ip} from adapter {target_adapter:?}", cfg.networking.ip_address);
                }
                cfg.networking.ip_address = Some(ip);
            }
            Ok(None) if cfg.networking.require_adapter => {
                anyhow::bail!("The network adapter \"{target_adapter}\" isn't connected. Connect it (e.g. turn your VPN on) and start the game again.");
            }
            Ok(None) => tracing::warn!("Adapter {target_adapter:?} not found; other players may not be able to join you"),
            Err(e) if cfg.networking.require_adapter => anyhow::bail!("Couldn't check network adapter \"{target_adapter}\": {e}"),
            Err(e) => tracing::warn!("Couldn't check adapter {target_adapter:?}: {e}"),
        }
    }

    if let Some(ip) = &cfg.networking.ip_address {
        info!("Enforcing {ip} for networking");
    }

    // if let Some(ref api_server) = cfg.api_server {
    crate::URL.set(cfg.api_server.clone()).map_err(|cfg| anyhow::anyhow!("Couldn't store api url {:?}", cfg))?;
    // }
    CONFIG.set(cfg).map_err(|cfg| anyhow::anyhow!("Couldn't store config {:?}", cfg))?;

    get().ok_or_else(|| anyhow::anyhow!("Config not loaded"))
}

pub fn get_config_path(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().join("uplay.toml")
}

pub fn default() -> Config {
    toml::from_str(DEFAULT_CONFIG).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_names_match_like_windows_would() {
        assert!(adapter_name_matches("Game VPN", "Game VPN"));
        assert!(adapter_name_matches("Game VPN", "game vpn"));
        assert!(adapter_name_matches("Game VPN", "Game VPN - 10.8.1.2"), "upstream launcher's picker format");
        assert!(adapter_name_matches("Ethernet - Virtual", "Ethernet - Virtual"), "only an IP suffix is stripped");
        assert!(!adapter_name_matches("Radmin VPN", "Game VPN"));
        assert!(!adapter_name_matches("Game VPN 2", "Game VPN"));
    }

    #[test]
    fn overrides_win_over_uplay_toml() {
        let mut cfg: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        cfg.user.cd_keys.push("KEEP-ME".into());
        let overrides: Overrides = toml::from_str(
            r#"
ConfigServer = "10.8.0.10"
ApiServer = "http://10.8.0.10:50051"
[User]
Username = "Kiwi"
Password = "secret"
[Networking]
Adapter = "Game VPN"
RequireAdapter = true
[Managed]
By = "Community Hub"
HelpText = "Open Community Hub and check the server is online."
"#,
        )
        .unwrap();
        overrides.apply(&mut cfg);
        assert_eq!(cfg.config_server.as_deref(), Some("10.8.0.10"));
        assert_eq!(cfg.api_server.as_str(), "http://10.8.0.10:50051/");
        assert_eq!(cfg.user.username, "Kiwi");
        assert_eq!(cfg.user.cd_keys, ["KEEP-ME"], "CD keys from uplay.toml survive");
        assert_eq!(cfg.networking.adapter.as_deref(), Some("Game VPN"));
        assert!(cfg.networking.require_adapter);
        let managed = cfg.managed.clone().unwrap();
        assert_eq!(managed.by, "Community Hub");
        assert_eq!(managed.help_text.as_deref(), Some("Open Community Hub and check the server is online."));
        assert!(!toml::to_string(&cfg).unwrap().contains("Community Hub"), "never written to uplay.toml");

        // An empty override file changes nothing.
        let before = cfg.clone();
        toml::from_str::<Overrides>("").unwrap().apply(&mut cfg);
        assert_eq!(cfg, before);
    }

    #[test]
    fn nat_settings_default_to_automatic_and_stay_out_of_the_file() {
        let mut cfg = super::default();
        assert_eq!(cfg.networking.nat, super::NatMode::Auto);
        assert!(cfg.networking.port_mapping);
        assert!(!toml::to_string(&cfg).unwrap().contains("Nat"), "defaults aren't written");
        cfg.networking.nat = super::NatMode::Relay;
        cfg.networking.port_mapping = false;
        let text = toml::to_string(&cfg).unwrap();
        let back: super::Config = toml::from_str(&text).unwrap();
        assert_eq!((back.networking.nat, back.networking.port_mapping), (super::NatMode::Relay, false));
        let old: super::Networking = toml::from_str("Adapter = \"Ethernet\"").unwrap();
        assert_eq!((old.nat, old.port_mapping), (super::NatMode::Auto, true));
    }

    #[test]
    fn upstream_configs_still_parse() {
        let cfg: Config = toml::from_str(
            r#"
ApiServer = "http://10.8.0.10:50051"
[User]
Username = "Nexus"
Password = "pw"
[Networking]
IpAddress = "10.8.1.2"
Adapter = "Game VPN - 10.8.1.2"
"#,
        )
        .unwrap();
        assert!(!cfg.networking.require_adapter);
        assert!(!toml::to_string(&cfg).unwrap().contains("RequireAdapter"), "not written unless set");
    }

    #[test]
    pub fn test_default_config() {
        let cfg: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        println!("{}", toml::to_string_pretty(&cfg).unwrap());
    }
}
