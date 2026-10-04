//! Windows: registry, drives, processes, known folders, network adapters.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;

use ::windows::core::PCWSTR;
use ::windows::Win32::Foundation::CloseHandle;
use ::windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
use ::windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use ::windows::Win32::Foundation::ERROR_MORE_DATA;
use ::windows::Win32::Foundation::ERROR_SUCCESS;
use ::windows::Win32::Foundation::HANDLE;
use ::windows::Win32::Foundation::WIN32_ERROR;
use ::windows::Win32::NetworkManagement::IpHelper::GetAdaptersAddresses;
use ::windows::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_ANYCAST;
use ::windows::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_DNS_SERVER;
use ::windows::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_MULTICAST;
use ::windows::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;
use ::windows::Win32::Networking::WinSock::AF_INET;
use ::windows::Win32::Networking::WinSock::SOCKADDR_IN;
use ::windows::Win32::Storage::FileSystem::GetDriveTypeW;
use ::windows::Win32::Storage::FileSystem::GetLogicalDrives;
use ::windows::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot;
use ::windows::Win32::System::Diagnostics::ToolHelp::Process32FirstW;
use ::windows::Win32::System::Diagnostics::ToolHelp::Process32NextW;
use ::windows::Win32::System::Diagnostics::ToolHelp::PROCESSENTRY32W;
use ::windows::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS;
use ::windows::Win32::System::Registry;
use ::windows::Win32::UI::Shell;

/// `DRIVE_FIXED` from GetDriveTypeW.
const DRIVE_FIXED: u32 = 3;

#[derive(Clone, Copy)]
enum Key {
    LocalMachine,
    CurrentUser,
}

/// A string value from the registry, also looking in the 32-bit view
/// (`WOW6432Node`) for `SOFTWARE\...` keys.
fn read_string(key: Key, subkey: &str, value: &str) -> Option<OsString> {
    let mut subkey = subkey.replace('/', "\\");
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let value = wide(value);
    let mut subkey_w = wide(&subkey);
    let mut buf = vec![0u16; 1024];
    loop {
        let mut size = (buf.len() * 2) as u32;
        let err = unsafe {
            Registry::RegGetValueW(
                match key {
                    Key::LocalMachine => Registry::HKEY_LOCAL_MACHINE,
                    Key::CurrentUser => Registry::HKEY_CURRENT_USER,
                },
                PCWSTR::from_raw(subkey_w.as_ptr()),
                PCWSTR::from_raw(value.as_ptr()),
                Registry::RRF_RT_REG_SZ | Registry::RRF_SUBKEY_WOW6432KEY,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        match err {
            ERROR_MORE_DATA => buf.resize(size as usize / 2 + 1, 0),
            ERROR_SUCCESS => {
                buf.truncate((size as usize / 2).saturating_sub(1));
                return Some(OsString::from_wide(&buf));
            }
            ERROR_FILE_NOT_FOUND if subkey.starts_with("SOFTWARE\\") && !subkey.starts_with("SOFTWARE\\WOW6432Node") => {
                subkey = subkey.replacen("SOFTWARE", "SOFTWARE\\WOW6432Node", 1);
                subkey_w = wide(&subkey);
            }
            _ => return None,
        }
    }
}

fn read_path(key: Key, subkey: &str, value: &str) -> Option<PathBuf> {
    read_string(key, subkey, value).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())
}

pub fn install_roots() -> Vec<PathBuf> {
    read_path(Key::LocalMachine, r"SOFTWARE\Ubisoft\Splinter Cell Blacklist", "installdir").into_iter().collect()
}

pub fn ubisoft_launcher_dir() -> Option<PathBuf> {
    read_path(Key::LocalMachine, r"SOFTWARE\Ubisoft\Launcher", "InstallDir")
}

fn steam_dirs() -> Vec<PathBuf> {
    [read_path(Key::CurrentUser, r"Software\Valve\Steam", "SteamPath"), read_path(Key::LocalMachine, r"SOFTWARE\Valve\Steam", "InstallPath")]
        .into_iter()
        .flatten()
        .collect()
}

/// Fixed drives, as `C:\`.
fn fixed_drives() -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", char::from(b'A' + i)))
        .filter(|root| {
            let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            let kind = unsafe { GetDriveTypeW(PCWSTR::from_raw(w.as_ptr())) };
            kind == DRIVE_FIXED
        })
        .map(PathBuf::from)
        .collect()
}

/// Steam libraries holding Proton prefixes: none on Windows.
pub fn steam_libraries() -> Vec<PathBuf> {
    Vec::new()
}

pub fn library_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for steam in steam_dirs() {
        let vdf = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).unwrap_or_default();
        let mut libraries = crate::game::steam_libraries(&vdf);
        libraries.push(steam);
        roots.extend(libraries.into_iter().map(|l| l.join("steamapps").join("common")));
    }
    roots.extend(ubisoft_launcher_dir().map(|d| d.join("games")));
    for drive in fixed_drives() {
        for sub in [
            r"Program Files (x86)\Steam\steamapps\common",
            r"Program Files\Steam\steamapps\common",
            r"SteamLibrary\steamapps\common",
            r"Steam\steamapps\common",
            r"Games\steamapps\common",
            r"Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games",
            r"Program Files\Ubisoft\Ubisoft Game Launcher\games",
            r"Games",
        ] {
            roots.push(drive.join(sub));
        }
    }
    roots.retain(|r| r.is_dir());
    roots
}

pub fn process_running(names: &[&str]) -> bool {
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while ok && !found {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
        found = names.iter().any(|n| n.eq_ignore_ascii_case(&exe));
        ok = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    let _ = unsafe { CloseHandle(snapshot) };
    found
}

pub fn roaming_app_data() -> Option<PathBuf> {
    unsafe {
        let path = Shell::SHGetKnownFolderPath(&Shell::FOLDERID_RoamingAppData, Shell::KNOWN_FOLDER_FLAG(0), HANDLE::default()).ok()?;
        let result = PathBuf::from(OsString::from_wide(path.as_wide()));
        ::windows::Win32::System::Com::CoTaskMemFree(Some(path.as_ptr() as *const _));
        Some(result)
    }
}

/// The game's `%APPDATA%`: this user's (under Wine, the prefix's).
pub fn game_roaming_dir(_game_dir: &std::path::Path) -> Option<PathBuf> {
    roaming_app_data()
}

/// Every adapter with an IPv4 address, by its friendly name (as the hook
/// matches `[Networking] Adapter`). Link-local addresses are left out.
pub fn adapters() -> Vec<(String, IpAddr)> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut buf = vec![0u8; 16 * 1024];
    let mut err = ERROR_BUFFER_OVERFLOW;
    // The list can grow between calls; retry a few times with the size asked for.
    for _ in 0..3 {
        let mut size = buf.len() as u32;
        err = WIN32_ERROR(unsafe { GetAdaptersAddresses(AF_INET.0 as u32, flags, None, Some(buf.as_mut_ptr().cast()), &mut size) });
        if err != ERROR_BUFFER_OVERFLOW {
            break;
        }
        buf.resize(size as usize, 0);
    }
    if err != ERROR_SUCCESS {
        tracing::warn!("GetAdaptersAddresses failed: {err:?}");
        return Vec::new();
    }
    let mut found = BTreeMap::new();
    let mut next = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while let Some(adapter) = unsafe { next.as_ref() } {
        let name = unsafe { adapter.FriendlyName.to_string() }.unwrap_or_default();
        let mut unicast = adapter.FirstUnicastAddress;
        while let Some(addr) = unsafe { unicast.as_ref() } {
            if addr.Address.iSockaddrLength as usize == std::mem::size_of::<SOCKADDR_IN>() {
                let sin = unsafe { &*addr.Address.lpSockaddr.cast::<SOCKADDR_IN>() };
                let ip = Ipv4Addr::from(u32::from_be(unsafe { sin.sin_addr.S_un.S_addr }));
                if !ip.is_link_local() && !name.is_empty() {
                    found.entry(name.clone()).or_insert(IpAddr::V4(ip));
                }
            }
            unicast = addr.Next;
        }
        next = adapter.Next;
    }
    found.into_iter().collect()
}
