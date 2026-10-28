//! Transportes P2P separados por plataforma, com endpoint restrito ao loopback.
#[cfg(windows)]
mod loopback;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
