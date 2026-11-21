//! On-demand MCP tools over newline-delimited JSON-RPC.

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
