use std::path::PathBuf;

fn main() {
    // Read at run time: cargo does not rerun a cached build script when the
    // checkout moves, and `env!` would freeze the old path into the binary,
    // making every later build look for fonts in the previous directory.
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR is set by cargo for build scripts");
    let root = PathBuf::from(manifest_dir).join("../..");
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
