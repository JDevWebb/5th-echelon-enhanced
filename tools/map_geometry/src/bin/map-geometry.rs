//! `map-geometry <game folder> <out folder> [MAP ...]`: writes each Spies vs Mercs map's
//! geometry (all of them when none is named) as `<MAP>.femg` for the Replay page's 3D view.
//! Built from your copy of the game: keep the files to yourself.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(game), Some(out)) = (args.first(), args.get(1)) else {
        eprintln!("Usage: map-geometry <game folder> <out folder> [MAP ...]");
        std::process::exit(2);
    };
    let Some(folder) = map_geometry::maps_folder(std::path::Path::new(game)) else {
        eprintln!("No map folder (src/SYSTEM/UMDs) under {game}");
        std::process::exit(1);
    };
    let maps = if args.len() > 2 { args[2..].to_vec() } else { map_geometry::svm_maps(&folder) };
    if let Err(e) = std::fs::create_dir_all(out) {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    }
    let mut failed = false;
    for name in maps {
        let started = std::time::Instant::now();
        let built = map_geometry::build(&folder, &name).and_then(|map| {
            let path = std::path::Path::new(out).join(format!("{name}.femg"));
            map_geometry::write(&map, std::fs::File::create(&path)?)?;
            Ok((map, path))
        });
        match built {
            Ok((map, path)) => println!(
                "{name}: {} placed ({} meshes, {} triangles), {} without geometry, {:.1} s -> {}",
                map.instances.len(),
                map.meshes.len(),
                map.meshes.iter().map(|m| m.triangles.len()).sum::<usize>(),
                map.missing,
                started.elapsed().as_secs_f32(),
                path.display()
            ),
            Err(e) => {
                eprintln!("{name}: {e}");
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
