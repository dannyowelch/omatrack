//! Write a tiny original module for trying the viewer and the replayer.
//!
//! ```text
//! cargo run --example write_showcase -- showcase.mod
//! cargo run -- showcase.mod
//! cargo run -- --render showcase.wav showcase.mod
//! ```
//!
//! The riff is a C major scale, not a copyrighted song. `.mod` files are
//! gitignored.

fn main() {
    let path = match std::env::args().nth(1) {
        Some(path) => path,
        None => {
            eprintln!("usage: write_showcase <file.mod>");
            std::process::exit(2);
        }
    };
    if let Err(err) = omatrack::demo::showcase().save(&path) {
        eprintln!("write_showcase: {err}");
        std::process::exit(1);
    }
}
