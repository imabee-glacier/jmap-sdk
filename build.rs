// CI passes the release version through CI_VERSION_JMAP_SDK (the tag is the source of truth);
// local builds fall back to the version in Cargo.toml.
fn main() {
    println!("cargo:rerun-if-env-changed=CI_VERSION_JMAP_SDK");
    let version = std::env::var("CI_VERSION_JMAP_SDK")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("CARGO_PKG_VERSION").ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=JMAP_SDK_VERSION={version}");
}
