fn main() {
    // Manifeste Windows 10 (necessaire aux fenetres enfants en transparence) pour le binaire de prototype.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bin=host-proto=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=host-proto=/MANIFESTINPUT:{dir}\\host-proto.manifest");
        println!("cargo:rerun-if-changed=host-proto.manifest");
    }
}
