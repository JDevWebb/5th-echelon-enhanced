//! l3d "Persistence" data (the game's `Src\Persistence.cpp`, reader DX11 `FUN_016517e0`): the
//! `.ass` header and each mesh's geometry. A struct is a run of records, each u32 size, u32
//! member (a name hash, [`hash`]), u32 flags, u32 type check, then its payload. flags & 2:
//! an array, u32 count then per element u32 size and records. Otherwise a nested struct when
//! its payload parses as records, else a plain value.
//!
//! The `.ass` (version 7): u32 version, u32 header size, the header (its `geometries`: name
//! hash, file position, size), then the data. A geometry's `sections` each have a
//! `vertex_stream` (`nb_vertices`, `stride`, `handle`: the vertices, a float3 position first)
//! and an `index_stream` (`nb_indices`, `handle`: u16 triangle lists, local to the section).

use crate::Error;

#[derive(Debug)]
pub enum Value {
    Raw(usize, usize),
    Struct(Vec<Record>, usize, usize),
    Array(Vec<Option<Vec<Record>>>),
}

#[derive(Debug)]
pub struct Record {
    pub member: u32,
    pub value: Value,
}

/// Name hashes: ~CRC32 (MSB first, polynomial 0x04C11DB7, from 0) of the lowercase name.
pub fn hash(name: &str) -> u32 {
    let mut c: u32 = 0;
    for b in name.to_lowercase().bytes() {
        let mut x = (c >> 24 ^ u32::from(b)) << 24;
        for _ in 0..8 {
            x = if x & 0x8000_0000 != 0 { (x << 1) ^ 0x04C1_1DB7 } else { x << 1 };
        }
        c = (c << 8) ^ x;
    }
    !c
}

fn u32_at(b: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
}

pub fn parse(b: &[u8], p: usize, end: usize) -> Option<Vec<Record>> {
    parse_at(b, p, end, 0)
}

fn parse_at(b: &[u8], mut p: usize, end: usize, depth: u32) -> Option<Vec<Record>> {
    let mut raw = Vec::new();
    while p < end {
        if p + 16 > end {
            return None;
        }
        let (size, member, flags) = (u32_at(b, p)? as usize, u32_at(b, p + 4)?, u32_at(b, p + 8)?);
        p += 16;
        if p + size > end {
            return None;
        }
        raw.push((member, flags, p, size));
        p += size;
    }
    Some(
        raw.into_iter()
            .map(|(member, flags, q, size)| Record {
                member,
                value: value(b, q, size, flags, depth + 1),
            })
            .collect(),
    )
}

fn value(b: &[u8], q: usize, size: usize, flags: u32, depth: u32) -> Value {
    if flags & 2 != 0 && size >= 4 {
        let n = u32_at(b, q).unwrap_or(0) as usize;
        let (mut items, mut r) = (Vec::new(), q + 4);
        for _ in 0..n {
            let Some(es) = u32_at(b, r).map(|s| s as usize).filter(|_| r + 4 <= q + size) else {
                break;
            };
            items.push(if depth < 12 { parse_at(b, r + 4, r + 4 + es, depth) } else { None });
            r += 4 + es;
        }
        if r == q + size {
            return Value::Array(items);
        }
    }
    if size >= 16 && depth < 12 {
        if let Some(sub) = parse_at(b, q, q + size, depth) {
            return Value::Struct(sub, q, size);
        }
    }
    Value::Raw(q, size)
}

pub fn get<'a>(records: &'a [Record], name: &str) -> Option<&'a Value> {
    let h = hash(name);
    records.iter().find(|r| r.member == h).map(|r| &r.value)
}

/// Where a value's bytes start (a plain value mistaken for a struct included).
fn start(v: &Value) -> Option<usize> {
    match v {
        Value::Raw(q, _) | Value::Struct(_, q, _) => Some(*q),
        Value::Array(_) => None,
    }
}

fn u32_of(b: &[u8], v: Option<&Value>) -> Option<u32> {
    u32_at(b, start(v?)?)
}

fn fields(v: Option<&Value>) -> Option<&[Record]> {
    match v? {
        Value::Struct(r, _, _) => Some(r),
        _ => None,
    }
}

/// The `.ass` header's geometries: name hash -> (position, size).
pub fn geometries(ass: &[u8]) -> Result<std::collections::HashMap<u32, (usize, usize)>, Error> {
    let size = u32_at(ass, 4).ok_or(Error::Format(".ass cut"))? as usize;
    let header = parse(ass, 8, 8 + size).ok_or(Error::Format("the .ass header doesn't parse"))?;
    let Some(Value::Array(items)) = get(&header, "geometries") else {
        return Err(Error::Format("the .ass has no geometries"));
    };
    Ok(items
        .iter()
        .flatten()
        .filter_map(|it| {
            Some((
                u32_of(ass, get(it, "name"))?,
                (u32_of(ass, get(it, "pos"))? as usize, u32_of(ass, get(it, "size"))? as usize),
            ))
        })
        .collect())
}

/// A geometry's sections: positions and triangles (as the file has them: clockwise from the
/// front, Direct3D).
pub fn sections(ass: &[u8], pos: usize, size: usize) -> Option<Vec<(Vec<[f32; 3]>, Vec<u16>)>> {
    let tree = parse(ass, pos, pos + size)?;
    let Some(Value::Array(secs)) = get(&tree, "sections") else { return None };
    let mut out = Vec::new();
    for sec in secs.iter().flatten() {
        let vs = fields(get(sec, "vertex_stream"))?;
        let is = fields(get(sec, "index_stream"))?;
        let (nv, stride) = (u32_of(ass, get(vs, "nb_vertices"))? as usize, u32_of(ass, get(vs, "stride"))? as usize);
        let vb = start(get(vs, "handle")?)?;
        if stride < 12 || nv > 1_000_000 {
            return None;
        }
        let mut verts = Vec::with_capacity(nv);
        for i in 0..nv {
            let at = vb + i * stride;
            let f = |k: usize| u32_at(ass, at + k * 4).map(f32::from_bits);
            verts.push([f(0)?, f(1)?, f(2)?]);
        }
        let ni = u32_of(ass, get(is, "nb_indices"))? as usize;
        let ib = start(get(is, "handle")?)?;
        let idx: Option<Vec<u16>> = (0..ni).map(|i| Some(u16::from_le_bytes(ass.get(ib + i * 2..ib + i * 2 + 2)?.try_into().ok()?))).collect();
        let idx = idx?;
        if idx.iter().any(|i| usize::from(*i) >= nv) {
            return None;
        }
        out.push((verts, idx));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_hash_as_the_game_does() {
        // From l3d_names.json (the game's own member names).
        assert_eq!(format!("{:08x}", hash("iut")), "a8e5cc57");
        assert_eq!(format!("{:08x}", hash("UsesLego")), "4cf3ac88");
    }
}
