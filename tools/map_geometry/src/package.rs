//! A LEAD (Unreal 2.5) package's tables and its objects' tagged properties.
//!
//! The summary (64 bytes): tag `C1832A9E`, u16 version 102, u16 licensee, then u32 flags,
//! ?, name count and offset, export count and offset, import count and offset, ... In a map
//! bundle the tables follow a copy of the summary back to back:
//! - names: u32 flags, compact length, the bytes;
//! - imports, 16 bytes each: class package, class name (names), outer (i32), object name;
//! - exports: compact class, compact super, i32 outer, u32 name, u64 flags, compact size,
//!   compact offset (when the size isn't 0).
//!
//! A name is u32: index in the low 18 bits, number above bit 19 (`Name_N` for number N + 1).
//! An object reference is an i32: negative an import, positive an export, 0 none.
//!
//! Properties are UE2 tags up to `None`: u32 name, a byte (type in the low 4 bits, size code
//! above, array flag in the top bit), a struct name for structs, the size (from the code, or
//! a u8, u16 or u32 after), an array index when flagged (a bool's flag is its value), then the
//! value. A few properties are cooked out: tag kept, no value ([`COOKED_OUT`]).

use crate::compact;
use crate::Error;

pub const TAG: [u8; 4] = [0xc1, 0x83, 0x2a, 0x9e];

/// Properties cooked out of shipped maps: a tag and no value bytes.
const COOKED_OUT: [&str; 16] = [
    "CatalogActorName",
    "CatalogItem",
    "Group",
    "OriginalName",
    "PreviousCatalogActorName",
    "PreviousCatalogItem",
    "SimulationNavMeshActor",
    "bIgnoreRigidBodyCheckForError",
    "bIsSimulationActor",
    "m_SpecificLocationList",
    "m_arrCheckMapIgnoredItem",
    "m_meshTmplName",
    "m_meshTmplVersion",
    "m_npcTmplName",
    "m_npcTmplVersion",
    "m_skeletalMeshNames",
];

pub struct Export {
    pub class: i32,
    pub outer: i32,
    pub name: u32,
    pub size: usize,
    pub offset: usize,
}

pub struct Package {
    pub names: Vec<String>,
    /// (outer, object name).
    pub imports: Vec<(i32, u32)>,
    pub exports: Vec<Export>,
}

fn u32_at(d: &[u8], p: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(d.get(p..p + 4).ok_or(Error::Format("package cut"))?.try_into().unwrap_or_default()))
}

impl Package {
    /// The tables after the summary at `at` (a bundle's copy).
    pub fn read(d: &[u8], at: usize) -> Result<Self, Error> {
        if d.get(at..at + 4) != Some(&TAG[..]) {
            return Err(Error::Format("not a package"));
        }
        let (nnames, nexports, nimports) = (u32_at(d, at + 16)? as usize, u32_at(d, at + 24)? as usize, u32_at(d, at + 32)? as usize);
        if nnames > 1_000_000 || nexports > 1_000_000 || nimports > 1_000_000 {
            return Err(Error::Format("package tables too large"));
        }
        let mut p = at + 64;
        let mut names = Vec::with_capacity(nnames);
        for _ in 0..nnames {
            let (n, q) = compact(d, p + 4)?;
            let n = usize::try_from(n).map_err(|_| Error::Format("bad name"))?;
            let bytes = d.get(q..q + n).ok_or(Error::Format("name cut"))?;
            names.push(bytes.iter().map(|b| char::from(*b)).collect());
            p = q + n;
        }
        let mut imports = Vec::with_capacity(nimports);
        for _ in 0..nimports {
            imports.push((u32_at(d, p + 8)? as i32, u32_at(d, p + 12)?));
            p += 16;
        }
        let mut exports = Vec::with_capacity(nexports);
        for _ in 0..nexports {
            let (class, q) = compact(d, p)?;
            let (_, q) = compact(d, q)?;
            let outer = u32_at(d, q)? as i32;
            let name = u32_at(d, q + 4)?;
            let (size, q) = compact(d, q + 16)?;
            let (offset, q) = if size != 0 { compact(d, q)? } else { (0, q) };
            p = q;
            exports.push(Export {
                class: i32::try_from(class).map_err(|_| Error::Format("bad class"))?,
                outer,
                name,
                size: usize::try_from(size).map_err(|_| Error::Format("bad size"))?,
                offset: usize::try_from(offset).map_err(|_| Error::Format("bad offset"))?,
            });
        }
        Ok(Self { names, imports, exports })
    }

    pub fn name(&self, v: u32) -> String {
        let (i, n) = ((v & 0x3ffff) as usize, v >> 19);
        let base = self.names.get(i).map_or("?", String::as_str);
        if n == 0 {
            base.to_string()
        } else {
            format!("{base}_{}", n - 1)
        }
    }

    pub fn object_name(&self, r: i32) -> String {
        match r {
            0 => String::from("None"),
            r if r < 0 => self.imports.get((-r - 1) as usize).map_or_else(|| "?".into(), |i| self.name(i.1)),
            r => self.exports.get((r - 1) as usize).map_or_else(|| "?".into(), |e| self.name(e.name)),
        }
    }

    pub fn class_name(&self, e: &Export) -> String {
        if e.class == 0 {
            String::from("Class")
        } else {
            self.object_name(e.class)
        }
    }

    /// `Package.Group.Object`, outermost first.
    pub fn path(&self, mut r: i32) -> String {
        let mut parts = Vec::new();
        while r != 0 && parts.len() < 16 {
            parts.push(self.object_name(r));
            r = if r < 0 {
                self.imports.get((-r - 1) as usize).map_or(0, |i| i.0)
            } else {
                self.exports.get((r - 1) as usize).map_or(0, |e| e.outer)
            };
        }
        parts.reverse();
        parts.join(".")
    }

    /// An object's tagged properties, in `u` (the rebuilt package): tried after a script
    /// state frame, then at each of the first bytes until a run of tags parses.
    pub fn properties(&self, u: &[u8], e: &Export) -> Option<Vec<Property>> {
        let (start, end) = (e.offset, e.offset + e.size);
        let mut tries = Vec::new();
        // A state frame: compact, compact, 12 bytes, compact.
        if let Some(p) = (|| {
            let (_, p) = compact(u, start).ok()?;
            let (_, p) = compact(u, p).ok()?;
            let (_, p) = compact(u, p + 12).ok()?;
            Some(p - start)
        })() {
            tries.push(p);
        }
        tries.extend(0..120);
        tries.into_iter().find_map(|s| self.props_at(u, start + s, end).filter(|r| !r.is_empty()))
    }

    fn props_at(&self, u: &[u8], mut p: usize, end: usize) -> Option<Vec<Property>> {
        let mut out = Vec::new();
        for _ in 0..600 {
            let v = u32_at(u, p).ok()?;
            p += 4;
            if (v & 0x3ffff) as usize >= self.names.len() {
                return None;
            }
            if v == 0 {
                return Some(out);
            }
            let info = *u.get(p)?;
            p += 1;
            let (kind, code, flagged) = (info & 0xf, (info >> 4) & 7, info & 0x80 != 0);
            if kind == 0 {
                return None;
            }
            let mut strukt = None;
            if kind == 10 {
                let s = u32_at(u, p).ok()?;
                p += 4;
                if (s & 0x3ffff) as usize >= self.names.len() {
                    return None;
                }
                strukt = Some(s);
            }
            let mut size = match code {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 12,
                4 => 16,
                5 => {
                    p += 1;
                    usize::from(*u.get(p - 1)?)
                }
                6 => {
                    p += 2;
                    usize::from(u16::from_le_bytes(u.get(p - 2..p)?.try_into().ok()?))
                }
                _ => {
                    p += 4;
                    u32_at(u, p - 4).ok()? as usize
                }
            };
            let mut index = 0;
            if kind != 3 && flagged {
                let b = *u.get(p)?;
                p += 1;
                if b & 0x80 != 0 {
                    if b & 0x40 != 0 {
                        index = u32::from_be_bytes(u.get(p - 1..p + 3)?.try_into().ok()?) & 0x3fff_ffff;
                        p += 3;
                    } else {
                        index = (u32::from(b & 0x7f) << 8) | u32::from(*u.get(p)?);
                        p += 1;
                    }
                } else {
                    index = u32::from(b);
                }
            }
            let name = self.name(v);
            if kind == 3 || COOKED_OUT.contains(&name.as_str()) {
                size = 0;
            }
            out.push(Property {
                name,
                kind,
                strukt: strukt.map(|s| self.name(s)),
                index,
                at: p,
                size,
                flag: flagged,
            });
            p += size;
            if p > end {
                return None;
            }
        }
        None
    }
}

pub struct Property {
    pub name: String,
    pub kind: u8,
    pub strukt: Option<String>,
    pub index: u32,
    pub at: usize,
    pub size: usize,
    /// A bool's value.
    pub flag: bool,
}

impl Property {
    pub fn object(&self, u: &[u8]) -> Option<i32> {
        (self.kind == 5).then(|| compact(u, self.at).ok()).flatten().and_then(|(r, _)| i32::try_from(r).ok())
    }

    pub fn float(&self, u: &[u8]) -> Option<f32> {
        (self.kind == 4 && self.size == 4)
            .then(|| Some(f32::from_le_bytes(u.get(self.at..self.at + 4)?.try_into().ok()?)))
            .flatten()
    }

    /// A Vector (3 floats) or Rotator (3 ints, as floats).
    pub fn triple(&self, u: &[u8]) -> Option<[f32; 3]> {
        if self.kind != 10 || self.size != 12 {
            return None;
        }
        let b = u.get(self.at..self.at + 12)?;
        let word = |i: usize| <[u8; 4]>::try_from(&b[i * 4..i * 4 + 4]).unwrap_or_default();
        match self.strukt.as_deref() {
            Some("Vector") => Some([0, 1, 2].map(|i| f32::from_le_bytes(word(i)))),
            #[allow(clippy::cast_precision_loss)]
            Some("Rotator") => Some([0, 1, 2].map(|i| i32::from_le_bytes(word(i)) as f32)),
            _ => None,
        }
    }
}
