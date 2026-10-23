//! Where the hooks' quick start (`QuickStartLobby`) hooks and calls the game: every function and
//! global it uses, found by signature. The hooks run this on the game as loaded and
//! `check_crash_fixes` on an executable on disk; nothing here writes anything.
//!
//! Each build ([`DX11`], [`DX9`]) is a list of items found in order: a few functions by a
//! signature that's in `.text` exactly once (absolute addresses and call targets are wildcards),
//! and everything else by a signature exactly once near the start of one of those, whose operand
//! is the item (a call's target, a function pointer, or a global). A function found that way must
//! start as that function does, and a global must be in the image but not in its code. A build
//! where any item isn't found that way is the other build or an unknown one: [`locate`] says why,
//! and the hooks change nothing.
//!
//! On the two known executables every item is found at the address it was worked out at (the
//! comments give them).

use super::crash_fixes::parse;
use super::crash_fixes::Image;

/// What the quick start uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// `UI::SceneStart::vf88`, the title screen's tick.
    SceneStartTick,
    /// One step of the menu state machine (`UI::SMI`), DX11 only.
    SmiStep,
    /// The travel to the Paladin hub after Start.
    HubTravel,
    /// Builds `UI::Task::OnlineMultiplayerStartupTask` and starts it on the UI task runner.
    OnlineStart,
    /// The online startup task's callback (queues the online menu).
    SmiPushOnline,
    /// `UI::SMIMatchmaking::vf88`, the online menu's tick.
    MatchmakingTick,
    /// The online menu's "select".
    MatchmakingSelect,
    /// A list entry's id.
    ListItemId,
    /// The engine singleton.
    GetEngine,
    /// Assigns a controller to the player.
    AssignController,
    /// `push <"menu">` in the Bink player's video gate.
    VideoGate,
    /// Globals: "Start pressed", `NetOnline`, `UI::SMI`, the UI task runner.
    StartPressed,
    NetOnline,
    Smi,
    TaskRunner,
}

/// How an item is found.
enum Find {
    /// The signature is in `.text` exactly once; the item is where it starts.
    Code(&'static [&'static str]),
    /// The signature is in the first [`NEAR`] bytes of `from` exactly once; the item is its
    /// operand at `operand`.
    In {
        from: Item,
        operand: usize,
        kind: Operand,
        /// How the function found starts (empty for a global).
        opens: &'static str,
        sig: &'static [&'static str],
    },
}

enum Operand {
    /// A `call rel32`'s target.
    Call,
    /// An absolute pointer to a function.
    Pointer,
    /// An absolute address of a global.
    Global,
}

/// How far into a function its references are looked for.
const NEAR: usize = 0x900;

/// One executable's items.
pub struct Build {
    pub name: &'static str,
    pub dx9: bool,
    items: &'static [(Item, Find)],
}

pub const DX11: Build = Build {
    name: "DX11",
    dx9: false,
    items: DX11_ITEMS,
};

pub const DX9: Build = Build {
    name: "DX9",
    dx9: true,
    items: DX9_ITEMS,
};

/// Where everything is, in one build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sites {
    pub name: &'static str,
    pub dx9: bool,
    pub scenestart_tick: usize,
    pub smi_step: Option<usize>,
    pub hub_travel: usize,
    pub online_start: usize,
    pub smi_push_online: usize,
    pub mm_tick: usize,
    pub mm_select: usize,
    pub list_item_id: usize,
    pub get_engine: usize,
    pub assign_controller: usize,
    /// The video gate's `jne` after its `menu` check, and the gate's own skip branch it's
    /// pointed at instead.
    pub video_gate_jne: usize,
    pub video_skip: usize,
    pub pressed: usize,
    pub net_online: usize,
    pub smi: usize,
    pub task_runner: usize,
}

/// The build `img` is, and where everything is in it.
pub fn locate(img: &Image) -> Result<Sites, String> {
    locate_build(img, &DX11).or_else(|dx11| locate_build(img, &DX9).map_err(|dx9| format!("not DX11 ({dx11}), not DX9 ({dx9})")))
}

/// Where everything is in `img`, as build `b`.
pub fn locate_build(img: &Image, b: &Build) -> Result<Sites, String> {
    let mut found: Vec<(Item, usize)> = Vec::new();
    let at = |found: &[(Item, usize)], item: Item| found.iter().find(|(i, _)| *i == item).map(|(_, va)| *va);
    let text = img.text_range();
    for (item, find) in b.items {
        let va = match find {
            Find::Code(sig) => img.find(&signature(sig)).map_err(|n| format!("{item:?} found {n} times, not once"))?,
            Find::In { from, operand, kind, opens, sig } => {
                let start = at(&found, *from).ok_or_else(|| format!("{item:?} before {from:?}"))?;
                let near = img.bytes(start, NEAR).or_else(|| img.bytes(start, text.end - start)).ok_or("past the code")?;
                let hit = super::crash_fixes::find_unique(near, &signature(sig)).map_err(|n| format!("{item:?} found {n} times near {from:?}, not once"))?;
                let op = start + hit + operand;
                let va = match kind {
                    Operand::Call => img.rel32_target(op),
                    Operand::Pointer | Operand::Global => img.u32(op),
                }
                .ok_or_else(|| format!("{item:?}'s operand"))?;
                match kind {
                    Operand::Global if text.contains(&va) || !img.image.contains(&(va + 3)) => return Err(format!("{item:?} {va:#010x} isn't a global")),
                    Operand::Call | Operand::Pointer if !opens_as(img, va, opens) => return Err(format!("{item:?} {va:#010x} doesn't start as expected")),
                    _ => va,
                }
            }
        };
        found.push((*item, va));
    }
    let get = |item: Item| at(&found, item).ok_or_else(|| format!("{item:?} missing"));
    let (video_gate_jne, video_skip) = video_gate(img, get(Item::VideoGate)?)?;
    Ok(Sites {
        name: b.name,
        dx9: b.dx9,
        scenestart_tick: get(Item::SceneStartTick)?,
        smi_step: at(&found, Item::SmiStep),
        hub_travel: get(Item::HubTravel)?,
        online_start: get(Item::OnlineStart)?,
        smi_push_online: get(Item::SmiPushOnline)?,
        mm_tick: get(Item::MatchmakingTick)?,
        mm_select: get(Item::MatchmakingSelect)?,
        list_item_id: get(Item::ListItemId)?,
        get_engine: get(Item::GetEngine)?,
        assign_controller: get(Item::AssignController)?,
        video_gate_jne,
        video_skip,
        pressed: get(Item::StartPressed)?,
        net_online: get(Item::NetOnline)?,
        smi: get(Item::Smi)?,
        task_runner: get(Item::TaskRunner)?,
    })
}

fn signature(parts: &[&str]) -> Vec<Option<u8>> {
    parse(&parts.join(" "))
}

/// The function at `va` is in the code and starts as `opens` says.
fn opens_as(img: &Image, va: usize, opens: &str) -> bool {
    let want = parse(opens);
    img.text_range().contains(&va) && img.bytes(va, want.len()).is_some_and(|have| have.iter().zip(&want).all(|(b, w)| w.is_none_or(|w| *b == w)))
}

/// The video gate's `jne` (20 bytes after the `push <"menu">` the signature found) and its skip
/// branch: right after the two `jne`s further on that go where the gate's goes
/// (`cmp ecx,edi / jne <far> / cmp esi,edi / jne <far>`). The pushed string must be `menu`.
fn video_gate(img: &Image, gate: usize) -> Result<(usize, usize), String> {
    const JNE_AT: usize = 20;
    const TAIL: &str = "3B CF 0F 85 ?? ?? ?? ?? 3B F7 0F 85 ?? ?? ?? ??";
    let menu = img.u32(gate + 1).ok_or("the video gate's string")?;
    if img.bytes(menu, 5) != Some(&b"menu\0"[..]) {
        return Err(format!("the video gate's string {menu:#010x} isn't \"menu\""));
    }
    let jne = gate + JNE_AT;
    let far = img.rel32_target(jne + 2).ok_or("the video gate's jump")?;
    let after = img.bytes(jne, 0x80).ok_or("the video gate's code")?;
    let tail = super::crash_fixes::find_unique(after, &parse(TAIL)).map_err(|n| format!("the video gate's skip branch found {n} times, not once"))?;
    let (first, second) = (jne + tail + 4, jne + tail + 12);
    if img.rel32_target(first) != Some(far) || img.rel32_target(second) != Some(far) {
        return Err("the video gate's skip branch doesn't match its jumps".into());
    }
    Ok((jne, jne + tail + 16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_signature_parses_whole() {
        for b in [&DX11, &DX9] {
            for (item, find) in b.items {
                let (sig, opens) = match find {
                    Find::Code(sig) => (sig, ""),
                    Find::In { sig, opens, .. } => (sig, *opens),
                };
                for part in sig.iter().copied().chain([opens]) {
                    assert_eq!(parse(part).len(), part.split_whitespace().count(), "{} {item:?}: {part:?}", b.name);
                }
                assert!(signature(sig).iter().filter(|b| b.is_some()).count() >= 6, "{} {item:?} is too loose", b.name);
            }
        }
    }

    #[test]
    fn another_build_is_neither() {
        let code = [0xCC; 0x400];
        let img = crate::crash_fixes::tests::image(&code, 0x40_1000, &[], 0x60_0000);
        let why = locate(&img).unwrap_err();
        assert!(
            why.starts_with("not DX11 (SceneStartTick found 0 times") && why.contains("not DX9 (SceneStartTick found 0 times"),
            "{why}"
        );
    }

    #[test]
    fn finds_the_video_gate_and_its_skip_branch() {
        // DX11's gate (`push <"menu">` at 0x42e979) up to its skip branch at 0x42e9ed, with the
        // string after it.
        let mut code = vec![0xCC; 0x100];
        #[rustfmt::skip]
        let gate = [
            0x68, 0x80, 0x10, 0x40, 0x00, 0x50, 0xE8, 0, 0, 0, 0, 0x83, 0xC4, 0x08, 0xF7, 0xD8, 0x1B, 0xC0, 0xF7, 0xD8,
            0x0F, 0x85, 0x01, 0x01, 0x00, 0x00,
        ];
        code[..gate.len()].copy_from_slice(&gate);
        // cmp ecx,edi / jne / cmp esi,edi / jne at gate+0x66, both to gate+0x11b as the gate's.
        code[0x66..0x76].copy_from_slice(&[0x3B, 0xCF, 0x0F, 0x85, 0xAD, 0x00, 0x00, 0x00, 0x3B, 0xF7, 0x0F, 0x85, 0xA5, 0x00, 0x00, 0x00]);
        code[0x80..0x85].copy_from_slice(b"menu\0");
        let img = crate::crash_fixes::tests::image(&code, 0x40_1000, &[], 0x60_0000);
        assert_eq!(video_gate(&img, 0x40_1000), Ok((0x40_1014, 0x40_1076)));
        code[0x80] = b'x';
        let img = crate::crash_fixes::tests::image(&code, 0x40_1000, &[], 0x60_0000);
        assert!(video_gate(&img, 0x40_1000).unwrap_err().contains("isn't \"menu\""));
    }
}

const DX11_ITEMS: &[(Item, Find)] = &[
    // 0x19d42a0
    (
        Item::SceneStartTick,
        Find::Code(&[
            "83 3D ?? ?? ?? ?? 00", // cmp dword <addr>,0x0
            "56",                   // push esi
            "8B F1",                // mov esi,ecx
            "7E 0A",                // jle <rel>
        ]),
    ),
    // 0x192bb60
    (
        Item::SmiStep,
        Find::Code(&[
            "55",                   // push ebp
            "8B EC",                // mov ebp,esp
            "51",                   // push ecx
            "56",                   // push esi
            "8B F1",                // mov esi,ecx
            "83 BE CC 02 00 00 00", // cmp dword [esi+0x2cc],0x0
        ]),
    ),
    // 0x197f610
    (
        Item::HubTravel,
        Find::Code(&[
            "55",                            // push ebp
            "8B EC",                         // mov ebp,esp
            "6A FF",                         // push 0xffffffff
            "68 ?? ?? ?? ??",                // push <addr>
            "64 A1 00 00 00 00",             // mov eax,fs:0x0
            "50",                            // push eax
            "81 EC A0 00 00 00",             // sub esp,0xa0
            "53",                            // push ebx
            "56",                            // push esi
            "57",                            // push edi
            "A1 ?? ?? ?? ??",                // mov eax,<addr>
            "33 C5",                         // xor eax,ebp
            "50",                            // push eax
            "8D 45 F4",                      // lea eax,[ebp-0xc]
            "64 A3 00 00 00 00",             // mov fs:0x0,eax
            "C7 81 68 05 00 00 0C 00 00 00", // mov dword [ecx+0x568],0xc
        ]),
    ),
    // 0x19c75d0
    (
        Item::OnlineStart,
        Find::Code(&[
            "55",                   // push ebp
            "8B EC",                // mov ebp,esp
            "6A FF",                // push 0xffffffff
            "68 ?? ?? ?? ??",       // push <addr>
            "64 A1 00 00 00 00",    // mov eax,fs:0x0
            "50",                   // push eax
            "83 EC 2C",             // sub esp,0x2c
            "A1 ?? ?? ?? ??",       // mov eax,<addr>
            "33 C5",                // xor eax,ebp
            "50",                   // push eax
            "8D 45 F4",             // lea eax,[ebp-0xc]
            "64 A3 00 00 00 00",    // mov fs:0x0,eax
            "8D 45 EC",             // lea eax,[ebp-0x14]
            "89 4D EC",             // mov dword [ebp-0x14],ecx
            "50",                   // push eax
            "8D 4D C8",             // lea ecx,[ebp-0x38]
            "C7 45 F0 ?? ?? ?? ??", // mov dword [ebp-0x10],<addr>
            "E8 ?? ?? ?? ??",       // call <rel>
            "C7 45 FC 00 00 00 00", // mov dword [ebp-0x4],0x0
        ]),
    ),
    // 0x19cdb90
    (
        Item::MatchmakingTick,
        Find::Code(&[
            "55",                   // push ebp
            "8B EC",                // mov ebp,esp
            "56",                   // push esi
            "8B F1",                // mov esi,ecx
            "80 BE E8 02 00 00 00", // cmp byte [esi+0x2e8],0x0
        ]),
    ),
    // 0x1a116c0
    (
        Item::MatchmakingSelect,
        Find::Code(&[
            "55",                // push ebp
            "8B EC",             // mov ebp,esp
            "6A FF",             // push 0xffffffff
            "68 ?? ?? ?? ??",    // push <addr>
            "64 A1 00 00 00 00", // mov eax,fs:0x0
            "50",                // push eax
            "83 EC 30",          // sub esp,0x30
            "53",                // push ebx
            "56",                // push esi
            "57",                // push edi
            "A1 ?? ?? ?? ??",    // mov eax,<addr>
            "33 C5",             // xor eax,ebp
            "50",                // push eax
            "8D 45 F4",          // lea eax,[ebp-0xc]
            "64 A3 00 00 00 00", // mov fs:0x0,eax
            "8B F1",             // mov esi,ecx
            "8B 86 A0 02 00 00", // mov eax,dword [esi+0x2a0]
        ]),
    ),
    // 0x42e979
    (
        Item::VideoGate,
        Find::Code(&[
            "68 ?? ?? ?? ??",    // push <addr>
            "50",                // push eax
            "E8 ?? ?? ?? ??",    // call <rel>
            "83 C4 08",          // add esp,0x8
            "F7 D8",             // neg eax
            "1B C0",             // sbb eax,eax
            "F7 D8",             // neg eax
            "0F 85 ?? ?? ?? ??", // jne <rel>
        ]),
    ),
    // 0x532210, at SceneStartTick+0x232
    (
        Item::GetEngine,
        Find::In {
            from: Item::SceneStartTick,
            operand: 1,
            kind: Operand::Call,
            opens: "83 3D ?? ?? ?? ?? 00 75 09 A1",
            sig: &[
                "E8 ?? ?? ?? ??", // call <rel>
                "8B 40 44",       // mov eax,dword [eax+0x44]
                "8B 10",          // mov edx,dword [eax]
            ],
        },
    ),
    // 0x675150, at SceneStartTick+0x228
    (
        Item::AssignController,
        Find::In {
            from: Item::SceneStartTick,
            operand: 6,
            kind: Operand::Call,
            opens: "55 8B EC 6A FF 68",
            sig: &[
                "6A 00",          // push 0x0
                "57",             // push edi
                "8B CB",          // mov ecx,ebx
                "E8 ?? ?? ?? ??", // call <rel>
            ],
        },
    ),
    // 0x1859b40, at MatchmakingSelect+0x53
    (
        Item::ListItemId,
        Find::In {
            from: Item::MatchmakingSelect,
            operand: 1,
            kind: Operand::Call,
            opens: "55 8B EC 56 8D 45 08 50",
            sig: &[
                "E8 ?? ?? ?? ??",    // call <rel>
                "89 86 E4 02 00 00", // mov dword [esi+0x2e4],eax
            ],
        },
    ),
    // 0x19b3970, at OnlineStart+0x2c
    (
        Item::SmiPushOnline,
        Find::In {
            from: Item::OnlineStart,
            operand: 6,
            kind: Operand::Pointer,
            opens: "55 8B EC 6A FF 68",
            sig: &[
                "8D 4D C8",             // lea ecx,[ebp-0x38]
                "C7 45 F0 ?? ?? ?? ??", // mov dword [ebp-0x10],<addr>
            ],
        },
    ),
    // 0x344da32, at SceneStartTick+0x1ec
    (
        Item::StartPressed,
        Find::In {
            from: Item::SceneStartTick,
            operand: 2,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "80 3D ?? ?? ?? ?? 00", // cmp byte <addr>,0x0
                "8B D8",                // mov ebx,eax
                "75 79",                // jne <rel>
            ],
        },
    ),
    // 0x3383df8, at SceneStartTick+0x217
    (
        Item::NetOnline,
        Find::In {
            from: Item::SceneStartTick,
            operand: 2,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "8B 0D ?? ?? ?? ??", // mov ecx,dword <addr>
                "8B 49 4C",          // mov ecx,dword [ecx+0x4c]
                "57",                // push edi
            ],
        },
    ),
    // 0x344db44, at SmiPushOnline+0x1e6
    (
        Item::Smi,
        Find::In {
            from: Item::SmiPushOnline,
            operand: 2,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "8B 0D ?? ?? ?? ??", // mov ecx,dword <addr>
                "85 C9",             // test ecx,ecx
                "75 3D",             // jne <rel>
            ],
        },
    ),
    // 0x344d640, at OnlineStart+0x42
    (
        Item::TaskRunner,
        Find::In {
            from: Item::OnlineStart,
            operand: 6,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "8D 4D C8",          // lea ecx,[ebp-0x38]
                "51",                // push ecx
                "8B 0D ?? ?? ?? ??", // mov ecx,dword <addr>
            ],
        },
    ),
];

const DX9_ITEMS: &[(Item, Find)] = &[
    // 0x513a60
    (
        Item::SceneStartTick,
        Find::Code(&[
            "83 3D ?? ?? ?? ?? 00", // cmp dword <addr>,0x0
            "56",                   // push esi
            "8B F1",                // mov esi,ecx
            "7E 0A",                // jle <rel>
        ]),
    ),
    // 0x5122c0
    (
        Item::OnlineStart,
        Find::Code(&[
            "55",                   // push ebp
            "8B EC",                // mov ebp,esp
            "6A FF",                // push 0xffffffff
            "68 ?? ?? ?? ??",       // push <addr>
            "64 A1 00 00 00 00",    // mov eax,fs:0x0
            "50",                   // push eax
            "83 EC 24",             // sub esp,0x24
            "53",                   // push ebx
            "56",                   // push esi
            "A1 ?? ?? ?? ??",       // mov eax,<addr>
            "33 C5",                // xor eax,ebp
            "50",                   // push eax
            "8D 45 F4",             // lea eax,[ebp-0xc]
            "64 A3 00 00 00 00",    // mov fs:0x0,eax
            "0F 57 C0",             // xorps xmm0,xmm0
            "B8 ?? ?? ?? ??",       // mov eax,<addr>
            "33 DB",                // xor ebx,ebx
            "89 45 DC",             // mov dword [ebp-0x24],eax
            "B8 01 00 00 00",       // mov eax,0x1
            "C7 45 D0 ?? ?? ?? ??", // mov dword [ebp-0x30],<addr>
            "89 5D D4",             // mov dword [ebp-0x2c],ebx
            "89 4D D8",             // mov dword [ebp-0x28],ecx
            "89 5D E0",             // mov dword [ebp-0x20],ebx
            "F3 0F 11 45 E4",       // movss dword [ebp-0x1c],xmm0
            "66 89 5D E8",          // mov WORD PTR [ebp-0x18],bx
            "89 45 EC",             // mov dword [ebp-0x14],eax
            "89 5D FC",             // mov dword [ebp-0x4],ebx
            "C7 45 D0 ?? ?? ?? ??", // mov dword [ebp-0x30],<addr>
            "66 89 5D F0",          // mov WORD PTR [ebp-0x10],bx
        ]),
    ),
    // 0x5c5c70
    (
        Item::MatchmakingTick,
        Find::Code(&[
            "55",                   // push ebp
            "8B EC",                // mov ebp,esp
            "56",                   // push esi
            "8B F1",                // mov esi,ecx
            "80 BE E8 02 00 00 00", // cmp byte [esi+0x2e8],0x0
        ]),
    ),
    // 0x5c4310
    (
        Item::MatchmakingSelect,
        Find::Code(&[
            "55",                // push ebp
            "8B EC",             // mov ebp,esp
            "6A FF",             // push 0xffffffff
            "68 ?? ?? ?? ??",    // push <addr>
            "64 A1 00 00 00 00", // mov eax,fs:0x0
            "50",                // push eax
            "83 EC 34",          // sub esp,0x34
            "53",                // push ebx
            "56",                // push esi
            "57",                // push edi
            "A1 ?? ?? ?? ??",    // mov eax,<addr>
            "33 C5",             // xor eax,ebp
            "50",                // push eax
            "8D 45 F4",          // lea eax,[ebp-0xc]
            "64 A3 00 00 00 00", // mov fs:0x0,eax
            "8B F9",             // mov edi,ecx
            "8B 8F A0 02 00 00", // mov ecx,dword [edi+0x2a0]
        ]),
    ),
    // 0x6e1f6d
    (
        Item::VideoGate,
        Find::Code(&[
            "68 ?? ?? ?? ??",    // push <addr>
            "50",                // push eax
            "E8 ?? ?? ?? ??",    // call <rel>
            "83 C4 08",          // add esp,0x8
            "F7 D8",             // neg eax
            "1B C0",             // sbb eax,eax
            "F7 D8",             // neg eax
            "0F 85 ?? ?? ?? ??", // jne <rel>
        ]),
    ),
    // 0x5143c0, at SceneStartTick+0x5c3
    (
        Item::HubTravel,
        Find::In {
            from: Item::SceneStartTick,
            operand: 1,
            kind: Operand::Call,
            opens: "55 8B EC 6A FF 68",
            sig: &[
                "E8 ?? ?? ?? ??", // call <rel>
                "BE 01 00 00 00", // mov esi,0x1
            ],
        },
    ),
    // 0x7db550, at SceneStartTick+0x237
    (
        Item::GetEngine,
        Find::In {
            from: Item::SceneStartTick,
            operand: 1,
            kind: Operand::Call,
            opens: "83 3D ?? ?? ?? ?? 00 75 09 A1",
            sig: &[
                "E8 ?? ?? ?? ??", // call <rel>
                "8B 40 44",       // mov eax,dword [eax+0x44]
                "8B 10",          // mov edx,dword [eax]
            ],
        },
    ),
    // 0x913dc0, at SceneStartTick+0x22d
    (
        Item::AssignController,
        Find::In {
            from: Item::SceneStartTick,
            operand: 6,
            kind: Operand::Call,
            opens: "55 8B EC 6A FF 68",
            sig: &[
                "6A 00",          // push 0x0
                "57",             // push edi
                "8B CB",          // mov ecx,ebx
                "E8 ?? ?? ?? ??", // call <rel>
            ],
        },
    ),
    // 0x48fce0, at MatchmakingSelect+0x74
    (
        Item::ListItemId,
        Find::In {
            from: Item::MatchmakingSelect,
            operand: 6,
            kind: Operand::Call,
            opens: "8B 41 10 56 85 C0 74",
            sig: &[
                "8D 55 F0",       // lea edx,[ebp-0x10]
                "33 F6",          // xor esi,esi
                "E8 ?? ?? ?? ??", // call <rel>
            ],
        },
    ),
    // 0x5157b0, at OnlineStart+0x2a
    (
        Item::SmiPushOnline,
        Find::In {
            from: Item::OnlineStart,
            operand: 1,
            kind: Operand::Pointer,
            opens: "55 8B EC 83 7D 08 00 75",
            sig: &[
                "B8 ?? ?? ?? ??", // mov eax,<addr>
                "33 DB",          // xor ebx,ebx
                "89 45 DC",       // mov dword [ebp-0x24],eax
            ],
        },
    ),
    // 0x3443123, at SceneStartTick+0x440
    (
        Item::StartPressed,
        Find::In {
            from: Item::SceneStartTick,
            operand: 5,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "55",                   // push ebp
                "8B EC",                // mov ebp,esp
                "80 3D ?? ?? ?? ?? 00", // cmp byte <addr>,0x0
            ],
        },
    ),
    // 0x32b5dc8, at SceneStartTick+0x1c1
    (
        Item::NetOnline,
        Find::In {
            from: Item::SceneStartTick,
            operand: 4,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "C2 04 00",       // ret 0x4
                "A1 ?? ?? ?? ??", // mov eax,<addr>
                "85 C0",          // test eax,eax
            ],
        },
    ),
    // 0x344323c, at SmiPushOnline+0x1bb
    (
        Item::Smi,
        Find::In {
            from: Item::SmiPushOnline,
            operand: 4,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "89 5D FC",       // mov dword [ebp-0x4],ebx
                "A1 ?? ?? ?? ??", // mov eax,<addr>
                "3B C3",          // cmp eax,ebx
            ],
        },
    ),
    // 0x34420c8, at OnlineStart+0x1c5
    (
        Item::TaskRunner,
        Find::In {
            from: Item::OnlineStart,
            operand: 6,
            kind: Operand::Global,
            opens: "",
            sig: &[
                "C6 45 FC 03",       // mov byte [ebp-0x4],0x3
                "8B 35 ?? ?? ?? ??", // mov esi,dword <addr>
            ],
        },
    ),
];
