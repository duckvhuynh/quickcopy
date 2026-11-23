fn main() {
    // App icon (Explorer, title bar) and the tray glyphs; see assets/.
    println!("cargo:rerun-if-changed=assets");
    embed_resource::compile("assets/quickcopy.rc", embed_resource::NONE).manifest_optional().unwrap();
}
