#[cfg(not(target_os = "linux"))]
compile_error!("jmap-sdk supports Linux only");
/// Release version: the git tag in CI, otherwise the Cargo.toml version (see build.rs).
pub const VERSION: &str = env!("JMAP_SDK_VERSION");
pub mod config;
pub mod jmap;
pub mod mail;
pub mod service;
pub mod store;
pub mod transport;

pub mod accounts;
