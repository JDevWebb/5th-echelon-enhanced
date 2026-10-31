//! This PC's hardware ids, for bans that hold for the PC as well as the identity (the 0.4.8
//! plan's F10). Only hashes leave the PC: each id is hashed here ([`hash`]), and the
//! coordinator keys the hashes again with a secret of its own before it keeps them.
//!
//! On Windows: the Windows install's MachineGuid, the SMBIOS system UUID and board serial,
//! and the fixed disks' serials. Under Wine or Proton (a Steam Deck, Linux), Windows' values
//! are Wine's own (one MachineGuid per prefix, SMBIOS and disks zeroed or made up), so the
//! Linux machine's are read instead, through the Z: drive Wine maps to `/`: the OS install's
//! machine-id and the NVMe and disk serials. A native Linux launcher reads the same at `/`.
//!
//! Placeholder values (all zeros, "To be filled by O.E.M.", "Default string"…) are never
//! sent: they're shared by many PCs, and must never tie two players together.

use std::path::Path;

use sha2::Digest;

/// One id, hashed: its kind and the hash (hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Id {
    pub kind: &'static str,
    pub hash: String,
}

/// What was found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hardware {
    pub ids: Vec<Id>,
    /// The PC is a virtual machine: its maker as the firmware names it ("VMware", "QEMU"…).
    pub vm: Option<String>,
}

/// The kinds of id, as sent.
pub const KINDS: [&str; 5] = ["machine-guid", "smbios-uuid", "board-serial", "disk-serial", "linux-machine-id"];

/// The most ids sent (disks are capped below that).
pub const MOST_IDS: usize = 12;
const MOST_DISKS: usize = 6;

/// This PC's ids. `under_wine`: a Windows build running under Wine or Proton.
pub fn collect(under_wine: bool) -> Hardware {
    #[cfg(windows)]
    {
        if under_wine {
            linux(Path::new("Z:\\"))
        } else {
            windows::collect()
        }
    }
    #[cfg(unix)]
    {
        let _ = under_wine;
        linux(Path::new("/"))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = under_wine;
        Hardware::default()
    }
}

/// The hash of `value` as an id of `kind`, once cleaned ([`clean`]): None for a placeholder.
pub fn hash(kind: &'static str, value: &str) -> Option<Id> {
    let value = clean(value)?;
    let mut h = sha2::Sha256::new();
    h.update(b"fes-hw-v1\0");
    h.update(kind.as_bytes());
    h.update(b"\0");
    h.update(value.as_bytes());
    Some(Id {
        kind,
        hash: h.finalize().iter().map(|b| format!("{b:02x}")).collect(),
    })
}

/// `value` trimmed and upper case, or None when it's a placeholder many PCs share: empty, too
/// short, one character over and over ("0000…", "FFFF…"), or a maker's filler text.
pub fn clean(value: &str) -> Option<String> {
    let v: String = value.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_uppercase();
    let plain: String = v.chars().filter(char::is_ascii_alphanumeric).collect();
    if plain.len() < 4 || plain.chars().all(|c| Some(c) == plain.chars().next()) {
        return None;
    }
    const FILLERS: [&str; 16] = [
        "TO BE FILLED",
        "DEFAULT STRING",
        "SYSTEM SERIAL",
        "SERIAL NUMBER",
        "BASE BOARD SERIAL",
        "BASEBOARD SERIAL",
        "NOT APPLICABLE",
        "NOT SPECIFIED",
        "NOT AVAILABLE",
        "NO ASSET",
        "CHASSIS SERIAL",
        "INVALID",
        "UNKNOWN",
        "NONE",
        "N/A",
        "O.E.M",
    ];
    const SHARED: [&str; 4] = ["03000200-0400-0500-0006-000700080009", "0123456789", "123456789", "1234567890"];
    if FILLERS.iter().any(|f| v.contains(f)) || SHARED.contains(&v.as_str()) || plain == "OEM" {
        return None;
    }
    Some(v)
}

/// The maker named by a virtual machine's firmware, if `vendor` and `product` are one's.
pub fn vm_maker(vendor: &str, product: &str) -> Option<String> {
    const MAKERS: [(&str, &str); 9] = [
        ("VMWARE", "VMware"),
        ("VIRTUALBOX", "VirtualBox"),
        ("INNOTEK", "VirtualBox"),
        ("QEMU", "QEMU"),
        ("KVM", "KVM"),
        ("XEN", "Xen"),
        ("PARALLELS", "Parallels"),
        ("BOCHS", "Bochs"),
        ("VIRTUAL MACHINE", "Hyper-V"),
    ];
    let both = format!("{vendor} {product}").to_uppercase();
    MAKERS.iter().find(|(needle, _)| both.contains(needle)).map(|(_, name)| (*name).to_string())
}

/// The Linux machine at `root` ("/", or "Z:\" under Wine): its machine-id, its NVMe and fixed
/// disks' serials, and whether the firmware says it's a virtual machine.
pub fn linux(root: &Path) -> Hardware {
    let read = |p: &Path| std::fs::read_to_string(p).ok().map(|s| s.trim().to_string());
    let at = |parts: &[&str]| parts.iter().fold(root.to_path_buf(), |p, part| p.join(part));
    let mut ids = Vec::new();
    if let Some(id) = read(&at(&["etc", "machine-id"])).or_else(|| read(&at(&["var", "lib", "dbus", "machine-id"]))) {
        ids.extend(hash("linux-machine-id", &id));
    }
    let mut serials = Vec::new();
    // NVMe controllers (a Steam Deck's own drive).
    for dir in list(&at(&["sys", "class", "nvme"])) {
        serials.extend(read(&dir.join("serial")));
    }
    // Other fixed disks that name a serial; never removable ones, SD cards, loop or RAM disks.
    for dir in list(&at(&["sys", "block"])) {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let skip = ["loop", "ram", "zram", "mmcblk", "sr", "dm-", "md", "nvme"].iter().any(|p| name.starts_with(p));
        if skip || read(&dir.join("removable")).as_deref() != Some("0") {
            continue;
        }
        serials.extend(read(&dir.join("device").join("serial")));
    }
    serials.sort();
    serials.dedup();
    ids.extend(serials.iter().take(MOST_DISKS).filter_map(|s| hash("disk-serial", s)));
    let vm = vm_maker(
        &read(&at(&["sys", "class", "dmi", "id", "sys_vendor"])).unwrap_or_default(),
        &read(&at(&["sys", "class", "dmi", "id", "product_name"])).unwrap_or_default(),
    );
    ids.truncate(MOST_IDS);
    Hardware { ids, vm }
}

/// The entries of `dir`, by name (none when it can't be read).
fn list(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    entries.sort();
    entries
}

/// What the SMBIOS tables say of the system and its board.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Smbios {
    /// The system UUID as text (its first three fields little-endian, as SMBIOS 2.6 says).
    pub uuid: Option<String>,
    pub vendor: String,
    pub product: String,
    pub board_serial: Option<String>,
}

/// Reads the SMBIOS structure table (`data`: what follows GetSystemFirmwareTable's 8-byte
/// header): type 1 (system: maker, product, UUID) and type 2 (board: serial).
pub fn parse_smbios(data: &[u8]) -> Smbios {
    let mut out = Smbios::default();
    let mut at = 0;
    while at + 4 <= data.len() {
        let (kind, len) = (data[at], usize::from(data[at + 1]));
        if len < 4 || at + len > data.len() {
            break;
        }
        let formatted = &data[at..at + len];
        // The strings follow, each NUL-ended, the set ended by another NUL.
        let mut end = at + len;
        let mut strings = Vec::new();
        while end < data.len() {
            let start = end;
            while end < data.len() && data[end] != 0 {
                end += 1;
            }
            if end == start {
                end += 1;
                if strings.is_empty() {
                    // No strings: the set is two NULs.
                    end += 1;
                }
                break;
            }
            strings.push(String::from_utf8_lossy(&data[start..end]).into_owned());
            end += 1;
        }
        let string = |offset: usize| {
            formatted
                .get(offset)
                .and_then(|i| usize::from(*i).checked_sub(1))
                .and_then(|i| strings.get(i).cloned())
                .unwrap_or_default()
        };
        match kind {
            1 if out.uuid.is_none() => {
                out.vendor = string(0x04);
                out.product = string(0x05);
                if let Some(u) = formatted.get(0x08..0x18) {
                    out.uuid = Some(format!(
                        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
                        u[3], u[2], u[1], u[0], u[5], u[4], u[7], u[6], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]
                    ));
                }
            }
            2 if out.board_serial.is_none() => out.board_serial = Some(string(0x07)).filter(|s| !s.is_empty()),
            127 => break,
            _ => {}
        }
        at = end;
    }
    out
}

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;

    use super::hash;
    use super::parse_smbios;
    use super::vm_maker;
    use super::Hardware;
    use super::MOST_DISKS;
    use super::MOST_IDS;

    type Handle = isize;
    const INVALID_HANDLE: Handle = -1;
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    // The 64-bit registry even from the 32-bit game: MachineGuid is there only.
    const RRF_SUBKEY_WOW6464KEY: u32 = 0x0001_0000;
    const RSMB: u32 = u32::from_be_bytes(*b"RSMB");
    const FILE_SHARE_READ_WRITE: u32 = 3;
    const OPEN_EXISTING: u32 = 3;
    const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(key: isize, subkey: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, len: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemFirmwareTable(provider: u32, id: u32, buffer: *mut c_void, size: u32) -> u32;
        fn CreateFileW(name: *const u16, access: u32, share: u32, security: *mut c_void, disposition: u32, flags: u32, template: Handle) -> Handle;
        fn DeviceIoControl(
            device: Handle,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn collect() -> Hardware {
        let mut ids = Vec::new();
        ids.extend(machine_guid().and_then(|g| hash("machine-guid", &g)));
        let smbios = smbios().map(|d| parse_smbios(&d)).unwrap_or_default();
        ids.extend(smbios.uuid.as_deref().and_then(|u| hash("smbios-uuid", u)));
        ids.extend(smbios.board_serial.as_deref().and_then(|s| hash("board-serial", s)));
        let mut serials = disk_serials();
        serials.sort();
        serials.dedup();
        ids.extend(serials.iter().take(MOST_DISKS).filter_map(|s| hash("disk-serial", s)));
        ids.truncate(MOST_IDS);
        Hardware {
            ids,
            vm: vm_maker(&smbios.vendor, &smbios.product),
        }
    }

    fn machine_guid() -> Option<String> {
        let (subkey, value) = (wide("SOFTWARE\\Microsoft\\Cryptography"), wide("MachineGuid"));
        let mut buf = [0u16; 128];
        let mut len = u32::try_from(std::mem::size_of_val(&buf)).ok()?;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        (status == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2)]).trim_end_matches('\0').to_string())
    }

    /// The raw SMBIOS table, without GetSystemFirmwareTable's 8-byte header.
    fn smbios() -> Option<Vec<u8>> {
        let size = unsafe { GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0) };
        if size < 8 || size > 1 << 20 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let got = unsafe { GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr().cast(), size) };
        if got != size {
            return None;
        }
        let len = u32::from_le_bytes(buf[4..8].try_into().ok()?) as usize;
        Some(buf[8..(8 + len).min(buf.len())].to_vec())
    }

    /// The fixed disks' serials (no USB, SD or virtual disks), as their drivers give them.
    fn disk_serials() -> Vec<String> {
        let mut serials = Vec::new();
        for n in 0..16 {
            let name = wide(&format!("\\\\.\\PhysicalDrive{n}"));
            // No access asked: the storage query needs none, nor administrator.
            let h = unsafe { CreateFileW(name.as_ptr(), 0, FILE_SHARE_READ_WRITE, std::ptr::null_mut(), OPEN_EXISTING, 0, 0) };
            if h == INVALID_HANDLE || h == 0 {
                continue;
            }
            // StorageDeviceProperty, PropertyStandardQuery.
            let query = [0u32; 3];
            let mut out = vec![0u8; 1024];
            let mut returned = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    h,
                    IOCTL_STORAGE_QUERY_PROPERTY,
                    query.as_ptr().cast(),
                    12,
                    out.as_mut_ptr().cast(),
                    1024,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            unsafe { CloseHandle(h) };
            if ok == 0 || returned < 32 {
                continue;
            }
            let u32_at = |o: usize| u32::from_le_bytes(out[o..o + 4].try_into().unwrap_or_default()) as usize;
            let (removable, serial_at, bus) = (out[10] != 0, u32_at(24), u32_at(28));
            // USB, SD, MMC, virtual and file-backed disks come and go.
            if removable || matches!(bus, 7 | 0xC | 0xD | 0xE | 0xF) || serial_at == 0 || serial_at >= returned as usize {
                continue;
            }
            let end = out[serial_at..returned as usize].iter().position(|b| *b == 0).map_or(returned as usize, |e| serial_at + e);
            serials.push(String::from_utf8_lossy(&out[serial_at..end]).trim().to_string());
            if serials.len() >= MOST_DISKS {
                break;
            }
        }
        serials
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_never_sent() {
        for v in [
            "",
            "  ",
            "0",
            "00000000-0000-0000-0000-000000000000",
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
            "To be filled by O.E.M.",
            "Default string",
            "System Serial Number",
            "03000200-0400-0500-0006-000700080009",
            "N/A",
            "None",
            "123456789",
            "OEM",
        ] {
            assert_eq!(clean(v), None, "{v:?}");
        }
        assert_eq!(clean(" s3r1al-42\0 ").as_deref(), Some("S3R1AL-42"));
    }

    #[test]
    fn the_same_value_hashes_the_same_whatever_its_case_and_kind_apart() {
        let a = hash("disk-serial", "abc123").unwrap();
        assert_eq!(a, hash("disk-serial", " ABC123 ").unwrap());
        assert_ne!(a.hash, hash("board-serial", "abc123").unwrap().hash);
        assert_eq!(a.hash.len(), 64);
    }

    #[test]
    fn virtual_machines_are_named() {
        assert_eq!(vm_maker("innotek GmbH", "VirtualBox").as_deref(), Some("VirtualBox"));
        assert_eq!(vm_maker("Microsoft Corporation", "Virtual Machine").as_deref(), Some("Hyper-V"));
        assert_eq!(vm_maker("Valve", "Jupiter"), None);
        assert_eq!(vm_maker("ASUS", "ROG STRIX"), None);
    }

    #[test]
    fn smbios_system_uuid_maker_and_board_serial() {
        let mut t = Vec::new();
        // Type 1, length 0x1b: maker 1, product 2, version 0, serial 3, UUID.
        let mut sys = vec![1u8, 0x1b, 0, 0, 1, 2, 0, 3];
        sys.extend([0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        sys.extend([0u8; 0x1b - 0x18]);
        t.extend(sys);
        t.extend(b"Valve\0Jupiter\0SN1\0\0");
        // Type 2, length 8: serial is string 1.
        t.extend([2u8, 8, 1, 0, 0, 0, 0, 1]);
        t.extend(b"BOARD-77\0\0");
        // A structure with no strings: two NULs.
        t.extend([3u8, 4, 2, 0, 0, 0]);
        t.extend([127u8, 4, 3, 0, 0, 0]);
        let s = parse_smbios(&t);
        assert_eq!(s.uuid.as_deref(), Some("00112233-4455-6677-8899-AABBCCDDEEFF"));
        assert_eq!((s.vendor.as_str(), s.product.as_str()), ("Valve", "Jupiter"));
        assert_eq!(s.board_serial.as_deref(), Some("BOARD-77"));
    }

    #[test]
    fn a_linux_machine_by_its_machine_id_and_disks() {
        let root = std::env::temp_dir().join(format!("fes-hwid-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mk = |p: &str, text: &str| {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        mk("etc/machine-id", "6b1f0e5c0d0e4f2a9c7d3b8a1e2f4c5d\n");
        mk("sys/class/nvme/nvme0/serial", "  STEAMDECK0042  \n");
        mk("sys/block/sda/removable", "0\n");
        mk("sys/block/sda/device/serial", "WD-123456\n");
        mk("sys/block/sdb/removable", "1\n");
        mk("sys/block/sdb/device/serial", "USBSTICK1\n");
        mk("sys/block/mmcblk0/removable", "0\n");
        mk("sys/block/mmcblk0/device/serial", "0x1234abcd\n");
        mk("sys/class/dmi/id/sys_vendor", "Valve\n");
        mk("sys/class/dmi/id/product_name", "Jupiter\n");
        let hw = linux(&root);
        let kinds: Vec<&str> = hw.ids.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, ["linux-machine-id", "disk-serial", "disk-serial"]);
        assert_eq!(hw.ids[1], hash("disk-serial", "STEAMDECK0042").unwrap());
        assert_eq!(hw.vm, None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
