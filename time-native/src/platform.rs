//! Platform-specific timer implementations

/// mac
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
use windows as platform;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "linux")]
mod linux;

// Fallback implementation for unsupported platforms
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
use fallback as platform;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod fallback;

// Re-export platform-specific types
pub use platform::*;
