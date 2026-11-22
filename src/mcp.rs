//! On-demand MCP tools over newline-delimited JSON-RPC.
//!
//! Expose an action with [`Entry::mcp`](crate::Entry::mcp) and serve its catalog
//! with [`run_stdio`]. The stdio host implements MCP revision `2025-11-25`.
//! After `initialize` and `notifications/initialized`, clients use `tools/list`
//! and `tools/call`. Namespaced actions appear as dot-separated tool names.
//!
//! Domain errors are tool results with `isError: true`. To receive progress,
//! supply a unique active `_meta.progressToken` with the call and register a
//! progress-capable binding. Progress notifications echo the token and start at
//! one. Closing stdin requests cancellation and lets accepted calls settle.

use crate::{
	Cancellation,
	application::Application,
	error::{Failure, FailureKind},
};

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

/// Runs the default server on the process streams until shutdown or disconnect.
/// The caller owns cancellation and process-level diagnostics.
pub fn run_stdio(application: &Application, shutdown: Cancellation) -> Result<(), Failure> {
	let (reader, writer) =
		stdio::open().map_err(|error| Failure::new(FailureKind::Unavailable, error))?;
	smol::block_on(serve(application, reader, writer, shutdown))
		.map_err(|error| Failure::new(FailureKind::OutputFailed, error))
}
