//! Token and sequence lifetime for one observed tool call.

use super::{FRAME_BYTES, delivery::Output};
use crate::{
	Cancellation,
	error::{Failure, FailureKind},
	jsonrpc::{Id, framing},
	structured::{ProgressSink, SendFuture},
};
use futures_lite::future;
use serde_json::{Value, json};
use std::sync::Mutex;

pub(super) struct Progress {
	pub token: Id,
	output: Output,
	sequence: Mutex<u64>,
	cancel: Cancellation,
	closed: Cancellation,
	withdrawn: Cancellation,
}

impl Progress {
	pub fn new(token: Id, output: Output, cancel: Cancellation) -> Self {
		Self {
			token,
			output,
			sequence: Mutex::new(0),
			cancel,
			closed: Cancellation::new(),
			withdrawn: Cancellation::new(),
		}
	}

	pub fn withdraw(&self) {
		self.withdrawn.cancel();
	}

	pub fn close(&self) {
		// A custom view can retain its context and await a slot on another executor.
		// Finish any enqueue already in progress before the host queues its reply.
		let _sequence = self.sequence.lock().unwrap();
		self.closed.cancel();
	}
}

impl ProgressSink for Progress {
	fn progress(&self, message: Value) -> SendFuture<'_> {
		Box::pin(async move {
			let stop = async {
				future::or(self.cancel.cancelled(), self.closed.cancelled()).await;
				None
			};
			let slot = future::or(stop, async { Some(self.output.progress_slot().await) }).await;
			let Some(slot) = slot else { return Ok(()) };

			let mut previous = self.sequence.lock().unwrap();
			if self.closed.is_cancelled() || self.cancel.is_cancelled() {
				return Ok(());
			}
			let progress = previous.checked_add(1).ok_or_else(|| {
				Failure::message(
					FailureKind::EncodingFailed,
					"MCP progress counter exhausted",
				)
			})?;
			let mut notification = json!({
				"jsonrpc": "2.0",
				"method": "notifications/progress",
				"params": { "progressToken": self.token.value(), "progress": progress },
			});

			if message.is_string() {
				notification["params"]["message"] = message;
			}
			let bytes = framing::encode(&notification, FRAME_BYTES)?;
			self.output
				.progress(bytes, self.withdrawn.clone(), slot)
				.map_err(|error| Failure::new(FailureKind::OutputFailed, error))?;
			*previous = progress;
			Ok(())
		})
	}
}
