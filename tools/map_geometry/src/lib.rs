//! A Spies vs Mercs map's geometry, read from the player's own copy of the game, for the 3D
//! replay viewer (the 0.5.0 plan's replays, way 1). Nothing from the game is in this crate or
//! shipped with it: it reads the map files where the game is installed and writes a file of
//! the geometry beside the person running it, who loads it into the Replay page (it stays in
//! their browser).
//!
//! A map is all static meshes (its BSP is next to nothing): every actor with a `StaticMesh`
//! (StaticMeshActors, props, light fixtures, items, vegetation), placed by its `Location`,
//! `Rotation`, `DrawScale` and `DrawScale3D`, with its mesh's geometry from the map's `.ass`
//! (looked up by the hash of `<mesh path>_l3d`). Positions are world units, the space a match
//! recording's are in. Worked out in the research notes (blacklist-re findings, "Whole-map
//! geometry").
//!
//! The file ([`write`]) is gzip around: `FEMG`, u32 version 1, the map's name (u8 length,
//! bytes), u32 mesh count, each mesh's u32 vertex count, u32 triangle count, f32 xyz each
//! vertex, u32 abc each triangle (counter-clockwise from the front, as OpenGL and three.js
//! take them; the game's are clockwise); then u32 instance count, each instance's u32 mesh,
//! u8 kind ([`Kind`]), and 12 f32: the 3x3 placement (columns: the mesh's x, y and z axes in
//! the world, scaled) and the position.

pub mod bundle;
pub mod epck;
pub mod package;
pub mod persist;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Format(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format(why) => write!(f, "{why}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// An Unreal compact index at `p`: its value and the position after it.
pub fn compact(d: &[u8], mut p: usize) -> Result<(i64, usize), Error> {
    let cut = Error::Format("compact index cut");
    let b = *d.get(p).ok_or(cut)?;
    p += 1;
    let negative = b & 0x80 != 0;
    let mut v = i64::from(b & 0x3f);
    if b & 0x40 != 0 {
        let mut shift = 6;
        loop {
            let b = *d.get(p).ok_or(Error::Format("compact index cut"))?;
            p += 1;
            v |= i64::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 || shift > 35 {
                break;
            }
        }
    }
    Ok((if negative { -v } else { v }, p))
}

/// What a placed mesh is, for the viewer to draw them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Structure = 0,
    Prop = 1,
    Light = 2,
    Item = 3,
    Vegetation = 4,
}

impl Kind {
    fn of(class: &str) -> Self {
        match class {
            "StaticMeshActor" => Self::Structure,
            c if c.starts_with("GI_LightFixture") => Self::Light,
            "VegetationActor" => Self::Vegetation,
            "GameItems" => Self::Item,
            _ => Self::Prop,
        }
    }
}

pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    /// Counter-clockwise from the front.
    pub triangles: Vec<[u32; 3]>,
}

pub struct Instance {
    pub mesh: u32,
    pub kind: Kind,
    /// Columns: the mesh's x, y and z axes in the world (scaled), then its position.
    pub placement: [f32; 12],
}

pub struct Map {
    pub name: String,
    pub meshes: Vec<Mesh>,
    pub instances: Vec<Instance>,
    /// Actors with a mesh this map's `.ass` has no geometry for.
    pub missing: usize,
}

/// The game's map folder (`src/SYSTEM/UMDs`) at or under `game`.
pub fn maps_folder(game: &Path) -> Option<PathBuf> {
    let mut queue = vec![(game.to_path_buf(), 0)];
    while let Some((dir, depth)) = queue.pop() {
        if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("UMDs")) && dir.is_dir() {
            return Some(dir);
        }
        if depth >= 4 {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            queue.extend(entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).map(|p| (p, depth + 1)));
        }
    }
    None
}

/// The Spies vs Mercs maps there: those named `ADV_...` with both a `.umd` and an `.ass`.
pub fn svm_maps(folder: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?.to_string();
            (p.extension()?.eq_ignore_ascii_case("umd") && stem.to_ascii_uppercase().starts_with("ADV_") && folder.join(format!("{stem}.ass")).exists()).then_some(stem)
        })
        .collect();
    out.sort();
    out
}

/// The UE2 rotation (pitch, yaw, roll; 65536 a turn) as the mesh's x, y and z axes.
fn axes(r: [f32; 3]) -> [[f32; 3]; 3] {
    let [p, y, ro] = r.map(|a| f64::from(a) * std::f64::consts::TAU / 65536.0);
    let (sp, cp, sy, cy, sr, cr) = (p.sin(), p.cos(), y.sin(), y.cos(), ro.sin(), ro.cos());
    #[allow(clippy::cast_possible_truncation)]
    let f = |v: [f64; 3]| v.map(|x| x as f32);
    [
        f([cp * cy, cp * sy, sp]),
        f([sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp]),
        f([-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp]),
    ]
}

/// Reads map `name` (e.g. `ADV_01`) from the game's map folder.
pub fn build(folder: &Path, name: &str) -> Result<Map, Error> {
    let umd = epck::unpack(&std::fs::read(folder.join(format!("{name}.umd")))?)?;
    let ass = epck::unpack(&std::fs::read(folder.join(format!("{name}.ass")))?)?;
    let bundle = bundle::Bundle::read(&umd)?;
    let level = bundle
        .files
        .iter()
        .find(|f| f.path.to_ascii_lowercase().ends_with(".unr"))
        .ok_or(Error::Format("the map has no level"))?;
    let (u, _) = bundle.rebuild(level);
    // The tables follow a copy of the level's summary in the bundle's data.
    let summary = u.get(..64).ok_or(Error::Format("the level is cut"))?;
    let mut at = bundle.start;
    let pkg = loop {
        let Some(found) = umd[at + 1..].windows(64).position(|w| w == summary) else {
            return Err(Error::Format("the level's tables weren't found"));
        };
        at += 1 + found;
        if let Ok(p) = package::Package::read(&umd, at) {
            if !p.exports.is_empty() {
                break p;
            }
        }
    };
    let index = persist::geometries(&ass)?;
    let mut meshes: Vec<Mesh> = Vec::new();
    let mut by_hash: HashMap<u32, Option<u32>> = HashMap::new();
    let mut instances = Vec::new();
    let mut missing = 0;
    for e in &pkg.exports {
        if e.size == 0 {
            continue;
        }
        let class = pkg.class_name(e);
        if class == "NavMeshActor" {
            continue;
        }
        let Some(props) = pkg.properties(&u, e) else { continue };
        let Some(mesh_ref) = props.iter().find(|p| p.name == "StaticMesh" && p.index == 0).and_then(|p| p.object(&u)).filter(|r| *r != 0) else {
            continue;
        };
        let get = |n: &str| props.iter().find(|p| p.name == n && p.index == 0);
        let location = get("Location").and_then(|p| p.triple(&u)).unwrap_or([0.0; 3]);
        let rotation = get("Rotation").and_then(|p| p.triple(&u)).unwrap_or([0.0; 3]);
        let scale = get("DrawScale").and_then(|p| p.float(&u)).unwrap_or(1.0);
        let scale3 = get("DrawScale3D").and_then(|p| p.triple(&u)).unwrap_or([1.0; 3]);
        let h = persist::hash(&format!("{}_l3d", pkg.path(mesh_ref)));
        let mesh = *by_hash.entry(h).or_insert_with(|| {
            let (pos, size) = *index.get(&h)?;
            let secs = persist::sections(&ass, pos, size)?;
            let mut m = Mesh {
                vertices: Vec::new(),
                triangles: Vec::new(),
            };
            for (verts, idx) in secs {
                let base = u32::try_from(m.vertices.len()).ok()?;
                m.vertices.extend(verts);
                // Clockwise in the file: turned round.
                m.triangles
                    .extend(idx.chunks_exact(3).map(|t| [base + u32::from(t[0]), base + u32::from(t[2]), base + u32::from(t[1])]));
            }
            meshes.push(m);
            u32::try_from(meshes.len() - 1).ok()
        });
        let Some(mesh) = mesh else {
            missing += 1;
            continue;
        };
        let ax = axes(rotation);
        let s = [scale * scale3[0], scale * scale3[1], scale * scale3[2]];
        let mut placement = [0f32; 12];
        for c in 0..3 {
            for r in 0..3 {
                placement[c * 3 + r] = ax[c][r] * s[c];
            }
        }
        placement[9..12].copy_from_slice(&location);
        instances.push(Instance {
            mesh,
            kind: Kind::of(&class),
            placement,
        });
    }
    Ok(Map {
        name: name.to_string(),
        meshes,
        instances,
        missing,
    })
}

/// The map's file for the viewer ([module docs](self)).
pub fn write(map: &Map, out: impl std::io::Write) -> std::io::Result<()> {
    let mut z = flate2::write::GzEncoder::new(std::io::BufWriter::new(out), flate2::Compression::default());
    z.write_all(b"FEMG")?;
    z.write_all(&1u32.to_le_bytes())?;
    let name = &map.name.as_bytes()[..map.name.len().min(255)];
    z.write_all(&[name.len() as u8])?;
    z.write_all(name)?;
    let n = |v: usize| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
    z.write_all(&n(map.meshes.len()))?;
    for m in &map.meshes {
        z.write_all(&n(m.vertices.len()))?;
        z.write_all(&n(m.triangles.len()))?;
        for v in &m.vertices {
            for x in v {
                z.write_all(&x.to_le_bytes())?;
            }
        }
        for t in &m.triangles {
            for i in t {
                z.write_all(&i.to_le_bytes())?;
            }
        }
    }
    z.write_all(&n(map.instances.len()))?;
    for i in &map.instances {
        z.write_all(&i.mesh.to_le_bytes())?;
        z.write_all(&[i.kind as u8])?;
        for x in i.placement {
            z.write_all(&x.to_le_bytes())?;
        }
    }
    z.finish()?.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_indices_read_as_unreal_writes_them() {
        assert_eq!(compact(&[0x05], 0).unwrap(), (5, 1));
        assert_eq!(compact(&[0x85], 0).unwrap(), (-5, 1));
        // 100 = 0b1100100: low 6 bits 0x24 with the "more" bit, then 1.
        assert_eq!(compact(&[0x64, 0x01], 0).unwrap(), (100, 2));
        assert!(compact(&[0x40], 0).is_err());
    }

    #[test]
    fn a_turn_of_yaw_is_a_turn_of_the_axes() {
        let a = axes([0.0, 16384.0, 0.0]);
        assert!((a[0][1] - 1.0).abs() < 1e-6 && a[0][0].abs() < 1e-6, "x faces y after a quarter turn: {a:?}");
        let i = axes([0.0; 3]);
        assert_eq!(i, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }
}
