use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        embed_manifest(new_manifest("miketsak.ThermoCompanion"))
            .expect("failed to embed Windows application manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
