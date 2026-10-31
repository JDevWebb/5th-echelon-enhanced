//! Says what a match recording holds: `replay-dump <file.fer>` (its header and a summary), or
//! with `--json`, every record as a JSON line (for a viewer, or a look at the numbers).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("Usage: replay-dump [--json] <recording.{}>", replay::EXTENSION);
        std::process::exit(2);
    };
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let (header, records) = match replay::read(std::io::BufReader::new(file)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    if json {
        println!("{}", serde_json::to_string(&header).unwrap_or_default());
        for r in &records {
            println!("{}", serde_json::to_string(r).unwrap_or_default());
        }
        return;
    }
    println!("{}", serde_json::to_string_pretty(&header).unwrap_or_default());
    println!("{}", serde_json::to_string_pretty(&replay::summary(&records)).unwrap_or_default());
}
