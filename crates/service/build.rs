//! Embeds data/photos.json (public product-photo URLs; an empty table when it is missing) so the
//! service can tell which models have an official photo. Missing file = no photos.

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it");
    let data = Path::new(&manifest).join("../../data");
    let photos = data.join("photos.json");
    // The directory changes when the file is created; the file when it is edited.
    println!("cargo:rerun-if-changed={}", data.display());
    if photos.exists() {
        println!("cargo:rerun-if-changed={}", photos.display());
    }
    let text = std::fs::read_to_string(&photos).unwrap_or_else(|_| "{}".to_string());
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets it")).join("photos.json");
    std::fs::write(out, text).expect("write OUT_DIR/photos.json");
}
