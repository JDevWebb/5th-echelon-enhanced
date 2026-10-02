//! Text a server sends (names, regions, messages), made safe to show: the
//! launcher and the overlay put it next to their own words, so it mustn't
//! run on for pages, break lines, or turn the text around it backwards.

/// Characters that reorder or hide the text around them: the bidi controls
/// (a name ending in U+202E shows what follows it reversed) and zero-width
/// marks.
fn sneaky(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// `text` without control characters (newlines included) or [`sneaky`]
/// ones, cut to `max` characters and trimmed.
pub fn clip(text: &str, max: usize) -> String {
    let kept: String = text.chars().filter(|c| !c.is_control() && !sneaky(*c)).take(max).collect();
    kept.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_and_cleans() {
        assert_eq!(clip("Kiwi Ops", 32), "Kiwi Ops");
        assert_eq!(clip("Tank\u{202e}gnp.exe", 32), "Tankgnp.exe");
        assert_eq!(clip("Server\nB\u{7}", 32), "ServerB");
        assert_eq!(clip(" zero\u{200b}width\u{2066} ", 32), "zerowidth");
        assert_eq!(clip(&"é".repeat(100), 10).chars().count(), 10);
        assert_eq!(clip("", 10), "");
    }
}
