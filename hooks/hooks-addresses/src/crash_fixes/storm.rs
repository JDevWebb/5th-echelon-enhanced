//! Storm's sent-packet tickets: the remove of an observer, and the two functions that call a
//! ticket's observers (the hooks' `storm_fix` has the whole story).

use super::Image;

/// The remove's start: `push ebp / mov ebp,esp / push ebx / mov ebx,ecx / mov eax,[ebx+0x24] /
/// mov ecx,[ebx+0x1c] / lea ecx,[eax+ecx*4] / push esi / cmp eax,ecx / jz / mov edx,[ebp+0xc] /
/// cmp [eax],edx / jz / add eax,4 / cmp eax,ecx / jnz / mov esi,[ebp+8] / push 1 / mov ecx,esi /
/// call <result> / mov eax,esi / pop esi / pop ebx / pop ebp / ret 8`; `None` is a wildcard.
#[rustfmt::skip]
pub const REMOVE: [Option<u8>; 54] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x53), Some(0x8B), Some(0xD9), Some(0x8B), Some(0x43), Some(0x24),
    Some(0x8B), Some(0x4B), Some(0x1C), Some(0x8D), Some(0x0C), Some(0x88), Some(0x56), Some(0x3B), Some(0xC1),
    Some(0x74), Some(0x0E), Some(0x8B), Some(0x55), Some(0x0C), Some(0x39), Some(0x10), Some(0x74), Some(0x1B),
    Some(0x83), Some(0xC0), Some(0x04), Some(0x3B), Some(0xC1), Some(0x75), Some(0xF5), Some(0x8B), Some(0x75),
    Some(0x08), Some(0x6A), Some(0x01), Some(0x8B), Some(0xCE), Some(0xE8), None, None, None, None,
    Some(0x8B), Some(0xC6), Some(0x5E), Some(0x5B), Some(0x5D), Some(0xC2), Some(0x08), Some(0x00),
];

/// The state change's start, up to its first branch on the new state; `None` is a wildcard.
#[rustfmt::skip]
pub const SET_STATE: [Option<u8>; 25] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x83), Some(0xEC), Some(0x08), // push ebp / mov ebp,esp / sub esp,8
    Some(0x8B), Some(0x45), Some(0x0C), Some(0x53), Some(0x56), Some(0x8B), Some(0xF1), // mov eax,[ebp+0xc] / push ebx / push esi / mov esi,ecx
    Some(0x8B), Some(0x56), Some(0x0C), Some(0x89), Some(0x55), Some(0xFC), // mov edx,[esi+0xc] / mov [ebp-4],edx
    Some(0x89), Some(0x46), Some(0x0C), Some(0x83), Some(0xF8), Some(0x03), // mov [esi+0xc],eax / cmp eax,3
];

/// The ack or loss's start, through its first read of the observer list.
#[rustfmt::skip]
pub const ON_RESULT: [Option<u8>; 23] = [
    Some(0x55), Some(0x8B), Some(0xEC), Some(0x8B), Some(0x45), Some(0x0C), // push ebp / mov ebp,esp / mov eax,[ebp+0xc]
    Some(0x53), Some(0x56), Some(0x57), Some(0x8B), Some(0xF1), Some(0x85), Some(0xC0), // push ebx/esi/edi / mov esi,ecx / test eax,eax
    Some(0x0F), Some(0x85), None, None, None, None, // jnz <lost>
    Some(0x8B), Some(0x46), Some(0x1C), // mov eax,[esi+0x1c]
    Some(0xC7), // mov dword [esi+0xc],2
];

/// The three functions, by address; each is detoured on its own.
#[derive(Debug)]
pub struct Sites {
    pub remove: Result<usize, String>,
    pub set_state: Result<usize, String>,
    pub on_result: Result<usize, String>,
}

pub fn locate(img: &Image) -> Sites {
    let find = |what: &str, signature: &[Option<u8>]| img.find(signature).map_err(|n| format!("{what} was found {n} times, not once"));
    Sites {
        remove: find("the ticket's observer remove", &REMOVE),
        set_state: find("the ticket's state change", &SET_STATE),
        on_result: find("the ticket's ack", &ON_RESULT),
    }
}
