use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

fn main() {
    // Re-run if the build parameter changes.
    println!("cargo:rerun-if-env-changed=RUSTRAPPER_EMBED");
    println!("cargo:rerun-if-env-changed=RUSTRAPPER_EMBED_DIR");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("embedded.rs");
    let mut f = fs::File::create(&dest).unwrap();

    // Collect (name, path) pairs from the build parameters.
    let mut entries: Vec<(String, String)> = Vec::new();

    // RUSTRAPPER_EMBED: comma-separated "name=path" pairs (or just "path",
    // in which case the file's basename is used as the name).
    if let Ok(val) = env::var("RUSTRAPPER_EMBED") {
        for part in val.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((name, path)) = part.split_once('=') {
                entries.push((name.trim().to_string(), path.trim().to_string()));
            } else {
                let p = Path::new(part);
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                entries.push((name, part.to_string()));
            }
        }
    }

    // RUSTRAPPER_EMBED_DIR: embed every file in the directory (name = basename).
    if let Ok(dir) = env::var("RUSTRAPPER_EMBED_DIR") {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() {
                    let name = p.file_name().unwrap().to_string_lossy().to_string();
                    entries.push((name, p.to_string_lossy().to_string()));
                }
            }
        }
    }

    // Generate a function returning the embedded files.
    writeln!(f, "pub fn embedded_files() -> Vec<(String, Vec<u8>)> {{").unwrap();
    writeln!(f, "    #[allow(unused_mut)]").unwrap();
    writeln!(f, "    let mut v = Vec::new();").unwrap();
    for (i, (name, path)) in entries.iter().enumerate() {
        // Use a unique include_bytes! per entry. The path must be absolute or
        // relative to the crate root; include_bytes! resolves relative to this
        // source file's directory, so convert to an absolute path.
        let abs = if Path::new(path).is_absolute() {
            path.clone()
        } else {
            env::current_dir().unwrap().join(path).to_string_lossy().to_string()
        };
        writeln!(f, "    v.push(({:?}.to_string(), include_bytes!({:?}).to_vec()));", name, abs).unwrap();
        let _ = i;
    }
    writeln!(f, "    v").unwrap();
    writeln!(f, "}}").unwrap();
}