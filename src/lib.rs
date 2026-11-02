#[cfg(not(target_os = "linux"))]
compile_error!("glacier-mail supports Linux only");
pub mod config;
pub mod jmap;
pub mod mail;
pub mod service;
pub mod store;
pub mod transport;

pub mod accounts;
