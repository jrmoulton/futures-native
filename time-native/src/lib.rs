//! Platform-native time utilities that don't depend on runtime-specific
//! reactors
//!
//! This provides timeout, sleep, and interval functionality using
//! platform-native timers, making it work with any async runtime (Tokio,
//! async-std, smol, etc.)

// Platform-specific timer implementations
mod platform;

// Time utility modules
pub mod interval;
pub mod sleep;
pub mod timeout;

// Re-export main types and functions for convenience
pub use {
    interval::{Interval, MissedTickBehavior, interval, interval_at},
    sleep::{Sleep, sleep},
    timeout::{Elapsed, Timeout, timeout},
};
