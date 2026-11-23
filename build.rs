use std::{env, fs, path::Path};

fn main() {
    // App icon (Explorer, title bar), the tray glyphs, version info and the
    // application manifest; see assets/.
    println!("cargo:rerun-if-changed=assets");
    // The target's OS, not the host's: build scripts run on the host.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // assets/quickcopy.rc includes version.h; rc.exe gets OUT_DIR on its
    // include path, so the version Explorer shows is the Cargo.toml one.
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = [0u16; 4];
    for (slot, n) in parts.iter_mut().zip(version.split(['.', '-', '+'])) {
        *slot = n.parse().unwrap_or(0);
    }
    let [a, b, c, d] = parts;
    let header = format!("#define QC_VERSION_BIN {a},{b},{c},{d}\n#define QC_VERSION_STR \"{version}\"\n");
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("version.h"), header).unwrap();

    embed_resource::compile("assets/quickcopy.rc", embed_resource::NONE).manifest_optional().unwrap();
}
