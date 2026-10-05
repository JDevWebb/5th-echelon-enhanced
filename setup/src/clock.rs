//! The time as the player's PC shows it: its UTC offset now, its time zone's short name, and
//! whether it shows 12 or 24-hour times. Rust's std has no local time, so these come from the
//! OS (Windows: `GetDynamicTimeZoneInformation` and `GetTimeFormatEx`; elsewhere `localtime_r`
//! and the locale's time format). Formatting is plain functions over them.

/// A PC's clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    /// Seconds east of UTC, now.
    pub offset: i32,
    /// "NZDT", "CET", or "UTC+13" when the system has no short name for it.
    pub zone: String,
    pub twelve_hour: bool,
}

const DAY: i64 = 86_400;

impl Clock {
    pub fn utc() -> Self {
        Self {
            offset: 0,
            zone: "UTC".into(),
            twelve_hour: false,
        }
    }

    /// This PC's clock, read once (the launcher asks for it every frame a notice shows).
    pub fn local() -> Self {
        static LOCAL: std::sync::Mutex<Option<(std::time::Instant, Clock)>> = std::sync::Mutex::new(None);
        let mut cached = LOCAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Read again now and then: daylight saving starts or ends while the launcher is open.
        if let Some((_, clock)) = cached.as_ref().filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(600)) {
            return clock.clone();
        }
        let clock = sys::local().unwrap_or_else(Self::utc);
        *cached = Some((std::time::Instant::now(), clock.clone()));
        clock
    }

    /// `at` (Unix seconds) as this clock shows it, with the zone: "9:46 am NZDT", and the day
    /// when it isn't today (`now`'s): "12:10 am tomorrow NZDT", "Wed 3:00 pm NZDT".
    pub fn time(&self, at: i64, now: i64) -> String {
        let local = at + i64::from(self.offset);
        let (day, today) = (local.div_euclid(DAY), (now + i64::from(self.offset)).div_euclid(DAY));
        let secs = local.rem_euclid(DAY);
        let (hour, minute) = (secs / 3600, secs % 3600 / 60);
        let time = if self.twelve_hour {
            let h = match hour % 12 {
                0 => 12,
                h => h,
            };
            format!("{h}:{minute:02} {}", if hour < 12 { "am" } else { "pm" })
        } else {
            format!("{hour:02}:{minute:02}")
        };
        match day - today {
            0 => format!("{time} {}", self.zone),
            1 => format!("{time} tomorrow {}", self.zone),
            _ => {
                // 1 January 1970 was a Thursday.
                let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][day.rem_euclid(7) as usize];
                format!("{weekday} {time} {}", self.zone)
            }
        }
    }
}

/// How long until something `secs` away, roughly: "now", "in ~16 min", "in ~2 h".
pub fn from_now(secs: i64) -> String {
    match secs {
        ..=29 => "now".into(),
        30..=89 => "in ~1 min".into(),
        90..=5399 => format!("in ~{} min", (secs + 30) / 60),
        _ => format!("in ~{} h", (secs + 1800) / 3600),
    }
}

/// "in ~16 min, about 9:46 am NZDT": relative first, then the clock time.
pub fn when(clock: &Clock, at: i64, now: i64) -> String {
    match from_now(at - now).as_str() {
        "now" => "now".into(),
        relative => format!("{relative}, about {}", clock.time(at, now)),
    }
}

/// "UTC+13", "UTC-3:30": a zone without a short name.
fn offset_name(offset: i32) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let (h, m) = (offset.abs() / 3600, offset.abs() % 3600 / 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

/// The short names for Windows' time zones (its names are long ones, "New Zealand Standard
/// Time"): standard and daylight saving, for the zones most players are in.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_zone(key: &str, daylight: bool) -> Option<&'static str> {
    let (standard, summer) = match key {
        "New Zealand Standard Time" => ("NZST", "NZDT"),
        "AUS Eastern Standard Time" | "Tasmania Standard Time" => ("AEST", "AEDT"),
        "E. Australia Standard Time" => ("AEST", "AEST"),
        "Cen. Australia Standard Time" => ("ACST", "ACDT"),
        "AUS Central Standard Time" => ("ACST", "ACST"),
        "W. Australia Standard Time" => ("AWST", "AWST"),
        "Tokyo Standard Time" => ("JST", "JST"),
        "Korea Standard Time" => ("KST", "KST"),
        "China Standard Time" => ("CST", "CST"),
        "Singapore Standard Time" => ("SGT", "SGT"),
        "India Standard Time" => ("IST", "IST"),
        "GMT Standard Time" => ("GMT", "BST"),
        "Greenwich Standard Time" => ("GMT", "GMT"),
        "W. Europe Standard Time" | "Romance Standard Time" | "Central Europe Standard Time" | "Central European Standard Time" => ("CET", "CEST"),
        "E. Europe Standard Time" | "GTB Standard Time" | "FLE Standard Time" => ("EET", "EEST"),
        "Russian Standard Time" => ("MSK", "MSK"),
        "Atlantic Standard Time" => ("AST", "ADT"),
        "Newfoundland Standard Time" => ("NST", "NDT"),
        "Eastern Standard Time" => ("EST", "EDT"),
        "Central Standard Time" => ("CST", "CDT"),
        "Mountain Standard Time" => ("MST", "MDT"),
        "US Mountain Standard Time" => ("MST", "MST"),
        "Pacific Standard Time" => ("PST", "PDT"),
        "Alaskan Standard Time" => ("AKST", "AKDT"),
        "Hawaiian Standard Time" => ("HST", "HST"),
        "E. South America Standard Time" => ("BRT", "BRT"),
        "UTC" => ("UTC", "UTC"),
        _ => return None,
    };
    Some(if daylight { summer } else { standard })
}

#[cfg(target_os = "windows")]
mod sys {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::SYSTEMTIME;
    use windows::Win32::Globalization::GetTimeFormatEx;
    use windows::Win32::Globalization::TIME_NOSECONDS;
    use windows::Win32::System::Time::GetDynamicTimeZoneInformation;
    use windows::Win32::System::Time::DYNAMIC_TIME_ZONE_INFORMATION;

    use super::Clock;

    /// 2: the zone is on daylight saving time now.
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;

    pub fn local() -> Option<Clock> {
        let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: fills the structure given.
        let id = unsafe { GetDynamicTimeZoneInformation(&mut info) };
        if id == u32::MAX {
            return None;
        }
        let daylight = id == TIME_ZONE_ID_DAYLIGHT;
        // UTC = local time + bias, in minutes.
        let bias = info.Bias + if daylight { info.DaylightBias } else { info.StandardBias };
        let offset = -bias * 60;
        let key = String::from_utf16_lossy(&info.TimeZoneKeyName).trim_end_matches('\0').to_string();
        let zone = super::windows_zone(&key, daylight).map_or_else(|| super::offset_name(offset), str::to_string);
        Some(Clock {
            offset,
            zone,
            twelve_hour: twelve_hour(),
        })
    }

    /// Whether the player's time format shows 13:00 as "1:00 PM" (their setting in Windows).
    fn twelve_hour() -> bool {
        let one_pm = SYSTEMTIME {
            wHour: 13,
            wYear: 2000,
            wMonth: 1,
            wDay: 1,
            ..Default::default()
        };
        let mut buffer = [0u16; 64];
        // SAFETY: the user's locale and format (null), and a buffer the call writes into.
        let n = unsafe { GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&one_pm), PCWSTR::null(), Some(&mut buffer)) };
        n > 0 && !String::from_utf16_lossy(&buffer[..n as usize]).contains("13")
    }
}

#[cfg(not(target_os = "windows"))]
mod sys {
    use super::Clock;

    pub fn local() -> Option<Clock> {
        let now: libc::time_t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs().try_into().ok()?;
        // SAFETY: a zeroed tm is valid; localtime_r fills it, and tm_zone (if set) points at a
        // name the C library keeps for the process.
        let tm = unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return None;
            }
            tm
        };
        let offset = i32::try_from(tm.tm_gmtoff).ok()?;
        let zone = if tm.tm_zone.is_null() {
            String::new()
        } else {
            // SAFETY: a C string from localtime_r, checked non-null.
            unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }.to_string_lossy().into_owned()
        };
        // Some zones have only numbers for a name ("+13").
        let zone = if zone.is_empty() || zone.starts_with(['+', '-']) { super::offset_name(offset) } else { zone };
        Some(Clock {
            offset,
            zone,
            twelve_hour: twelve_hour(),
        })
    }

    /// Whether the locale's time format is a 12-hour one (`%I`, `%l` or `%r`).
    fn twelve_hour() -> bool {
        static TWELVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *TWELVE.get_or_init(|| {
            // SAFETY: setlocale for LC_TIME from the environment, then the format string
            // nl_langinfo hands back for it (copied at once).
            unsafe {
                libc::setlocale(libc::LC_TIME, c"".as_ptr());
                let format = libc::nl_langinfo(libc::T_FMT);
                !format.is_null() && {
                    let format = std::ffi::CStr::from_ptr(format).to_string_lossy();
                    format.contains("%I") || format.contains("%l") || format.contains("%r")
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nz() -> Clock {
        Clock {
            offset: 13 * 3600,
            zone: "NZDT".into(),
            twelve_hour: true,
        }
    }

    // 2026-10-05 08:30 UTC: 21:30 in New Zealand (NZDT, UTC+13).
    const NOW: i64 = 1_791_189_000;

    #[test]
    fn times_in_the_players_zone() {
        assert_eq!(nz().time(NOW, NOW), "9:30 pm NZDT");
        assert_eq!(nz().time(NOW + 16 * 60, NOW), "9:46 pm NZDT");
        // Past midnight there: tomorrow, though it's the same day in UTC.
        assert_eq!(nz().time(NOW + 3 * 3600, NOW), "12:30 am tomorrow NZDT");
        let cet = Clock {
            offset: 3600,
            zone: "CET".into(),
            twelve_hour: false,
        };
        assert_eq!(cet.time(NOW, NOW), "09:30 CET");
        let edt = Clock {
            offset: -4 * 3600,
            zone: "EDT".into(),
            twelve_hour: true,
        };
        assert_eq!(edt.time(NOW, NOW), "4:30 am EDT");
        assert_eq!(edt.time(NOW + 8 * 3600, NOW), "12:30 pm EDT");
        assert_eq!(edt.time(NOW + 3 * DAY, NOW), "Thu 4:30 am EDT");
    }

    #[test]
    fn relative_first() {
        assert_eq!(from_now(-5), "now");
        assert_eq!(from_now(60), "in ~1 min");
        assert_eq!(from_now(16 * 60), "in ~16 min");
        assert_eq!(from_now(2 * 3600 + 100), "in ~2 h");
        assert_eq!(when(&nz(), NOW + 16 * 60, NOW), "in ~16 min, about 9:46 pm NZDT");
        assert_eq!(when(&nz(), NOW, NOW), "now");
    }

    #[test]
    fn zones_without_a_short_name() {
        assert_eq!(offset_name(13 * 3600), "UTC+13");
        assert_eq!(offset_name(-(3 * 3600 + 1800)), "UTC-3:30");
        assert_eq!(windows_zone("New Zealand Standard Time", true), Some("NZDT"));
        assert_eq!(windows_zone("W. Europe Standard Time", false), Some("CET"));
        assert_eq!(windows_zone("Somewhere Standard Time", false), None);
        let local = Clock::local();
        assert!(!local.zone.is_empty());
    }
}
