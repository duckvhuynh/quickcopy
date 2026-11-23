fn main() {
    // App icon (Explorer, title bar) and the tray glyphs; see assets/.
    println!("cargo:rerun-if-changed=assets");
    // The target's OS, not the host's: build scripts run on the host.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed_resource::compile("assets/quickcopy.rc", embed_resource::NONE).manifest_optional().unwrap();
}
