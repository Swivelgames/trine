//! Exclusive native stdio adapters. Do not concurrently use the process streams
//! through another reader/writer while these adapters are alive.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::open;
#[cfg(windows)]
pub use windows::open;
