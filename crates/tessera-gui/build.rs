use std::path::PathBuf;

fn main() {
    let asset =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts/tessera-unifont.otf.gz");
    println!("cargo:rerun-if-changed={}", asset.display());
    if !asset.exists() {
        panic!(
            "missing embedded font asset: {}\n\
             Run `python3 tools/fetch_unifont.py` from the repository root first.",
            asset.display()
        );
    }
}
