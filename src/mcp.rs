//! On-demand MCP tools over newline-delimited JSON-RPC.

mod calls;
mod delivery;
mod host;
mod progress;
mod protocol;
mod session;
mod tool;

pub use crate::jsonrpc::stdio;
pub use host::serve;

const REVISION: &str = "2025-11-25";
const FRAME_BYTES: usize = 1 << 20;
const QUEUED_FRAMES: usize = 16;
