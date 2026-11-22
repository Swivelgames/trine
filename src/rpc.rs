//! Live, bounded JSON-RPC stdio profile over the shared action catalog.
//!
//! Use [`run_stdio`] to serve a catalog on process stdin and stdout. Send an
//! `invoke` request with a unique ID, an action path (an array of strings) and an
//! input object. Set `params.observe` to true to request progress. Each progress
//! notification carries the invocation ID and a sequence starting at one.
//! Send a `cancel` notification with `params.id` to request cancellation.
//!
//! The host retains active IDs and a bounded history of settled IDs. A duplicate
//! produces a `rejected` notification and leaves the original response correlated
//! with its request. Clean input EOF drains accepted work. See [`Config`] and
//! [`serve`] to change limits or supply your own asynchronous streams.

use std::io;

use crate::{
	Cancellation,
	application::Application,
	error::{Failure, FailureKind},
	jsonrpc::framing,
};

mod host;
mod outbox;
mod protocol;
mod sink;

pub use crate::jsonrpc::stdio;
pub use host::{HostReport, serve};

/// Resource settings are local host policy, not protocol negotiation.
#[derive(Clone, Debug)]
pub struct Config {
	/// Maximum bytes accepted in a single input frame.
	pub input_frame_bytes: usize,
	/// Maximum bytes allowed in an encoded output frame.
	pub output_frame_bytes: usize,
	/// Maximum UTF-8 byte length of a string request ID.
	pub id_bytes: usize,
	/// Number of settled request IDs retained for duplicate detection.
	pub history: usize,
	/// Maximum concurrent invocations admitted on one connection.
	pub active_invocations: usize,
	/// Maximum ordinary output messages buffered by the host.
	pub output_messages: usize,
	/// Byte budget for buffered ordinary output.
	pub output_bytes: usize,
	/// Maximum buffered rejection messages.
	pub rejection_messages: usize,
	/// Separate byte budget for buffered rejection messages.
	pub rejection_bytes: usize,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			input_frame_bytes: 1 << 20,
			output_frame_bytes: 1 << 20,
			id_bytes: 256,
			history: 4096,
			active_invocations: 16,
			output_messages: 64,
			output_bytes: 4 << 20,
			rejection_messages: 16,
			rejection_bytes: 64 << 10,
		}
	}
}

impl Config {
	/// Check that limits are nonzero, fit their counters and can accommodate required frames.
	/// Invalid combinations return an invalid-input I/O error.
	pub fn validate(&self) -> io::Result<()> {
		let event_capacity = self
			.active_invocations
			.checked_add(self.output_messages)
			.and_then(|count| count.checked_add(2));

		if event_capacity.is_none()
			|| self.input_frame_bytes == 0
			|| self.output_frame_bytes < 512
			|| self.id_bytes == 0
			|| self.history == 0
			|| self.active_invocations == 0
			|| self.output_messages == 0
			|| self.rejection_messages == 0
			|| self.output_bytes <= self.output_frame_bytes
			|| self.rejection_bytes < 512
		{
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"RPC limits must be nonzero and budgets must admit complete frames",
			));
		}

		if self.id_bytes.saturating_mul(6).saturating_add(512) > self.output_frame_bytes {
			return Err(io::Error::new(
				io::ErrorKind::InvalidInput,
				"output limit must admit an escaped ID and framework error",
			));
		}

		Ok(())
	}
}

/// Runs the default server on the process streams until shutdown or disconnect.
/// The caller owns cancellation and process-level diagnostics.
pub fn run_stdio(application: &Application, shutdown: Cancellation) -> Result<(), Failure> {
	let (reader, writer) =
		stdio::open().map_err(|error| Failure::new(FailureKind::Unavailable, error))?;
	let report = smol::block_on(serve(
		application,
		reader,
		writer,
		Config::default(),
		shutdown,
		|_| {},
	))
	.map_err(|error| Failure::new(FailureKind::OutputFailed, error))?;

	if let Some(message) = report.failure {
		return Err(Failure::message(FailureKind::Unavailable, message));
	}
	Ok(())
}
