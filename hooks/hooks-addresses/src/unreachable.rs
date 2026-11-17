//! Storm's report of a peer it couldn't reach (0.5.5's JC4; the hooks' `unreachable.rs` has the
//! story).
//!
//! `Storm::session`'s peer-failure report (DX11 VA `0x20ace20`, DX9 `0x2085930`) is
//! `thiscall(session, result*, context, peer*, code*, message*, a6, a7) -> result*`, `ret 0x1c`:
//! `peer` is the peer's `PeerDescriptor`, `code` the error pair it fails with. The places that
//! found the peer unreachable pass `CONNECTION_FAILED`'s pair (DX11 `0x34c3870`), which is read
//! from one of them, a lost peer's handler (DX11 `0x20b45c0`, its push at `0x20b4626`).

use crate::crash_fixes::Image;

/// The report's start: `push ebp / mov ebp,esp / sub esp,0x10 / push ebx / push esi / mov esi,ecx /
/// push edi / lea eax,[esi+0x2a0] / push eax / mov eax,[ebp+0x10] / push eax / call <find> /
/// mov ebx,[ebp+0xc] / mov edi,eax / test edi,edi / jz <none> / mov ecx,[edi+0x1dc]`.
#[rustfmt::skip]
pub const REPORT: [Option<u8>; 46] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x83), Some(0xEC), Some(0x10), Some(0x53), Some(0x56),
    Some(0x8B), Some(0xF1), Some(0x57), Some(0x8D), Some(0x86), Some(0xA0), Some(0x02), Some(0x00),
    Some(0x00), Some(0x50), Some(0x8B), Some(0x45), Some(0x10), Some(0x50), Some(0xE8), None,
    None, None, None, Some(0x8B), Some(0x5D), Some(0x0C), Some(0x8B), Some(0xF8),
    Some(0x85), Some(0xFF), Some(0x0F), Some(0x84), None, None, None, None,
    Some(0x8B), Some(0x8F), Some(0xDC), Some(0x01), Some(0x00), Some(0x00),
];

/// The lost peer's handler calling it: `mov eax,[ebp+8] / push 0 / push 1 / lea edx,[ebp-0x10] /
/// push edx / push <CONNECTION_FAILED> / push edi / push eax / lea ecx,[ebp+0xc] / push ecx /
/// mov ecx,esi / call`.
#[rustfmt::skip]
pub const LOST_PEER_CALL: [Option<u8>; 25] = [
    Some(0x8B), Some(0x45), Some(0x08), Some(0x6A), Some(0x00), Some(0x6A), Some(0x01), Some(0x8D),
    Some(0x55), Some(0xF0), Some(0x52), Some(0x68), None, None, None, None,
    Some(0x57), Some(0x50), Some(0x8D), Some(0x4D), Some(0x0C), Some(0x51), Some(0x8B), Some(0xCE),
    Some(0xE8),
];

/// Where the report is, and `CONNECTION_FAILED`'s pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sites {
    pub report: usize,
    pub connection_failed: usize,
}

pub fn locate(img: &Image) -> Result<Sites, String> {
    let report = img.find(&REPORT).map_err(|n| format!("the peer-failure report was found {n} times, not once"))?;
    let call = img.find(&LOST_PEER_CALL).map_err(|n| format!("the lost peer's report was found {n} times, not once"))?;
    // It calls the report found.
    if img.rel32_target(call + LOST_PEER_CALL.len()) != Some(report) {
        return Err("the lost peer's handler calls another function than the report found".into());
    }
    let connection_failed = img.u32(call + 12).ok_or("the lost peer's push isn't readable")?;
    if !img.image.contains(&connection_failed) {
        return Err(format!("CONNECTION_FAILED's pair {connection_failed:#010x} is outside the game"));
    }
    Ok(Sites { report, connection_failed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crash_fixes::tests::image;

    #[test]
    fn finds_the_report_and_the_code_its_caller_passes() {
        let text_va = 0x40_1000;
        let mut text = vec![0x90u8; 0x200];
        let report = 0x10;
        for (i, b) in REPORT.iter().enumerate() {
            text[report + i] = b.unwrap_or(0);
        }
        let call = 0x100;
        for (i, b) in LOST_PEER_CALL.iter().enumerate() {
            text[call + i] = b.unwrap_or(0);
        }
        text[call + 12..call + 16].copy_from_slice(&0x0040_1180u32.to_le_bytes());
        let after = text_va + call + LOST_PEER_CALL.len() + 4;
        let rel = u32::try_from(text_va + report).unwrap().wrapping_sub(u32::try_from(after).unwrap());
        text[call + 25..call + 29].copy_from_slice(&rel.to_le_bytes());
        let img = image(&text, text_va, &[], 0x60_0000);
        assert_eq!(
            locate(&img),
            Ok(Sites {
                report: text_va + report,
                connection_failed: 0x0040_1180
            })
        );
    }
}
