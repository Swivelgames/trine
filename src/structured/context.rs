//! Invocation-scoped output lifetime and transcript/live sink coordination.

use super::{Payload, Transcript};
use crate::error::{Failure, FailureKind};
use serde_json::Value;
use std::{
	future::Future,
	pin::Pin,
	sync::{Arc, Mutex},
};

pub(crate) type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send + 'a>>;

pub(crate) trait LiveSink: Send + Sync {
	fn progress(&self, value: Value) -> SendFuture<'_>;

	fn terminal(&self, payload: Payload) -> SendFuture<'_>;
}

#[derive(Default)]
struct State {
	transcript: Transcript,
	finishing: bool,
	closed: bool,
	violation: Option<Failure>,
	terminal: bool,
}

// Retained contexts cannot outlive a failed factory or an unwinding action.
pub(crate) struct CloseSink(StructuredContext);

impl Drop for CloseSink {
	fn drop(&mut self) {
		let _ = self.0.close(false);
	}
}

/// A sink belongs to exactly one invocation. It cannot select a request or interface.
#[derive(Clone)]
pub struct StructuredContext {
	state: Arc<Mutex<State>>,
	observe: bool,
	live: Option<Arc<dyn LiveSink>>,
}

impl StructuredContext {
	pub(crate) fn close_on_drop(&self) -> CloseSink {
		CloseSink(self.clone())
	}

	pub(crate) fn new(observe: bool) -> Self {
		Self {
			state: Arc::new(Mutex::new(State::default())),
			observe,
			live: None,
		}
	}

	pub(crate) fn with_live(observe: bool, sink: Arc<dyn LiveSink>) -> Self {
		let mut context = Self::new(observe);
		context.live = Some(sink);
		context
	}

	pub async fn progress(&self, value: Value) -> Result<(), Failure> {
		{
			let mut state = self
				.state
				.lock()
				.map_err(|_| "presentation lock poisoned")?;

			if state.closed || state.finishing {
				return Err("progress is closed".into());
			}
			if !self.observe {
				return Ok(());
			}

			if self.live.is_none() {
				state.transcript.progress.push(value);
				return Ok(());
			}
		}

		self.live.as_ref().unwrap().progress(value).await
	}

	pub async fn terminal(&self, payload: Payload) -> Result<(), Failure> {
		{
			let mut state = self
				.state
				.lock()
				.map_err(|_| "presentation lock poisoned")?;

			let error = if state.closed {
				Some("presentation is closed")
			} else if !state.finishing {
				Some("terminal payload is only allowed during finish")
			} else if state.terminal {
				Some("duplicate terminal payload")
			} else {
				None
			};

			if let Some(error) = error {
				let error = Failure::message(FailureKind::ContractViolation, error);
				state.violation = Some(error.clone());
				return Err(error);
			}

			if self.live.is_none() {
				state.transcript.terminal = Some(payload);
				state.terminal = true;
				return Ok(());
			}
		}

		self.live.as_ref().unwrap().terminal(payload).await?;
		self.state
			.lock()
			.map_err(|_| "presentation lock poisoned")?
			.terminal = true;
		Ok(())
	}

	pub(crate) fn begin_finish(&self) {
		self.state.lock().unwrap().finishing = true;
	}

	pub(crate) fn close(&self, require_terminal: bool) -> Result<(), Failure> {
		let mut state = self
			.state
			.lock()
			.map_err(|_| "presentation lock poisoned")?;
		state.closed = true;

		if let Some(error) = &state.violation {
			return Err(error.clone());
		}

		if require_terminal && !state.terminal {
			return Err(Failure::message(
				FailureKind::ContractViolation,
				"view supplied no terminal payload",
			));
		}
		Ok(())
	}

	pub(crate) fn framework_diagnostic(&self, text: &str) -> Result<(), Failure> {
		let mut state = self.state.lock().map_err(|_| {
			Failure::message(FailureKind::OutputFailed, "presentation lock poisoned")
		})?;
		if state.transcript.terminal.is_some() {
			state.transcript.diagnostics.push(text.into());
			return Ok(());
		}

		state.transcript.terminal = Some(Payload::FrameworkFailure {
			kind: FailureKind::Panicked,
			message: text.into(),
		});
		Ok(())
	}

	pub(crate) fn fallback_diagnostic(&self, text: &str) -> Result<(), Failure> {
		self.state
			.lock()
			.map_err(|_| Failure::message(FailureKind::OutputFailed, "presentation lock poisoned"))?
			.transcript
			.diagnostics
			.push(text.into());
		Ok(())
	}

	pub(crate) fn transcript(&self) -> Transcript {
		self.state.lock().unwrap().transcript.clone()
	}
}
