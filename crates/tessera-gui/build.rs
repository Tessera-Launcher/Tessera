use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assets = [
        (
            "assets/fonts/tessera-pixel8.ttf.gz",
            "tools/fetch_pixel_font.py",
        ),
        (
            "assets/fonts/tessera-unifont.otf.gz",
            "tools/fetch_unifont.py",
        ),
    ];

    for (asset, script) in assets {
        let path = root.join(asset);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.exists() {
            panic!(
                "missing embedded font asset: {}\n\
                 Run `python3 {script}` from the repository root first.",
                path.display()
            );
        }
    }
}
