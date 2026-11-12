//! Audit tool: import every corpus fixture (Moho Pro 14.4 native saves) and print what the
//! import couldn't bring across — the live difference list against Moho. Run with the corpus
//! beside the workspace or `MAOHOU_FIXTURES` pointing at its `maop/` directory:
//!
//! `cargo run -p anicraft-mohoproj-import --example audit`

use anicraft_mohoproj_import::import_path;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn corpus() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MAOHOU_FIXTURES") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let beside = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../maop-corpus/maop");
    beside.is_dir().then_some(beside)
}

fn main() {
    let Some(dir) = corpus() else {
        eprintln!("no maop-corpus at MAOHOU_FIXTURES or ../../maop-corpus");
        std::process::exit(1);
    };
    let mut union: BTreeMap<String, usize> = BTreeMap::new();
    let mut files = 0;
    let mut extras = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plan/moho/samples/CharWalk.mohoproj")];
    for e in std::fs::read_dir(&dir).expect("corpus dir reads") {
        let Ok(e) = e else { continue };
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("moho") {
            extras.push(p);
        }
    }
    extras.sort();
    for p in &extras {
        if !p.is_file() {
            continue;
        }
        files += 1;
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        match import_path(p) {
            Ok((doc, report)) => {
                let layers = doc.scenes.iter().map(|s| s.timeline.layers.len()).sum::<usize>();
                println!("{name}: {} layers, {} sounds; {}", layers, doc.sounds.len(), report.summary());
                for (k, n) in &report.skipped {
                    *union.entry(k.clone()).or_default() += n;
                }
                for note in &report.notes {
                    println!("    note: {note}");
                }
                if !report.missing_fonts.is_empty() {
                    println!("    fonts: {:?}", report.missing_fonts);
                }
            }
            Err(e) => println!("{name}: FAILED: {e}"),
        }
    }
    println!("\n== union of not-yet-supported, over {files} documents ==");
    for (k, n) in &union {
        println!("{n:>4}  {k}");
    }
}
