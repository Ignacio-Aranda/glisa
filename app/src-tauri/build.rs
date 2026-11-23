fn main() {
    // data/photos.json (public product-photo URLs, written by hand) is embedded; an empty
    // table when it doesn't exist yet.
    let src = std::path::Path::new("../../data/photos.json");
    println!("cargo:rerun-if-changed=../../data/photos.json");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("photos.json");
    let text = std::fs::read_to_string(src).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(out, text).unwrap();
    tauri_build::build()
}
