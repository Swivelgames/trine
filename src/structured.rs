//! In-process presentation harness. These values are not RPC or MCP envelopes.

use crate::error::{Failure, FailureKind};
use crate::{application::presentation::ErasedView, view::View};
use serde::Serialize;
use serde_json::Value;
use std::{
	future::Future,
	marker::PhantomData,
	num::NonZeroUsize,
	pin::Pin,
	sync::{Arc, Mutex},
};

#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
	Success(Value),
	DomainFailure(Value),
	/// Explicit application error presentation for the live RPC adapter.
	RpcError {
		code: i64,
		message: String,
		data: Value,
	},
	FrameworkFailure {
		kind: FailureKind,
		message: String,
	},
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
	pub diagnostics: Vec<String>,
	pub progress: Vec<Value>,
	pub terminal: Option<Payload>,
}

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

type Factory<O, E, P> =
	dyn Fn(StructuredContext) -> Result<ErasedView<O, E, P>, Failure> + Send + Sync;

/// A typed factory plus explicit observation configuration.
pub struct StructuredBinding<O, E, P> {
	pub(crate) factory: Arc<Factory<O, E, P>>,
	pub(crate) capacity: Option<NonZeroUsize>,
}

impl<O: Sync + 'static, E: Sync + 'static, P: Send + 'static> StructuredBinding<O, E, P> {
	pub fn custom<V, F>(factory: F) -> Self
	where
		V: View<O, E, P, Completion = ()> + 'static,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
		F: Fn(StructuredContext) -> Result<V, Failure> + Send + Sync + 'static,
	{
		Self {
			factory: Arc::new(move |context| factory(context).map(ErasedView::new)),
			capacity: None,
		}
	}

	/// Custom progress presentation is opt-in, and the invocation must also request it.
	pub fn observe(mut self, capacity: NonZeroUsize) -> Self {
		self.capacity = Some(capacity);
		self
	}
}

impl<O: Serialize + Sync + 'static, E: Serialize + Sync + 'static, P: Send + 'static>
	StructuredBinding<O, E, P>
{
	pub fn serde() -> Self {
		Self::custom(|context| {
			Ok(SerdeView::<O, E, P> {
				context,
				mapper: None,
				marker: PhantomData,
			})
		})
	}

	pub fn serde_with_progress(
		mapper: impl Fn(P) -> Result<Value, Failure> + Send + Sync + 'static,
		capacity: NonZeroUsize,
	) -> Self {
		let mapper = Arc::new(mapper);
		Self::custom(move |context| {
			Ok(SerdeView::<O, E, P> {
				context,
				mapper: Some(mapper.clone()),
				marker: PhantomData,
			})
		})
		.observe(capacity)
	}
}

type Mapper<P> = dyn Fn(P) -> Result<Value, Failure> + Send + Sync;

struct SerdeView<O, E, P> {
	context: StructuredContext,
	mapper: Option<Arc<Mapper<P>>>,
	marker: PhantomData<fn() -> (O, E)>,
}

impl<O: Serialize + Sync, E: Serialize + Sync, P: Send + 'static> View<O, E, P>
	for SerdeView<O, E, P>
{
	type Completion = ();
	type Error = Failure;

	async fn progress(&mut self, event: P) -> Result<(), Failure> {
		if let Some(mapper) = &self.mapper {
			self.context.progress(mapper(event)?).await?;
		}
		Ok(())
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<(), Failure> {
		let payload = match outcome {
			Ok(value) => Payload::Success(
				serde_json::to_value(value)
					.map_err(|e| Failure::new(FailureKind::EncodingFailed, e))?,
			),
			Err(error) => Payload::DomainFailure(
				serde_json::to_value(error)
					.map_err(|e| Failure::new(FailureKind::EncodingFailed, e))?,
			),
		};

		self.context.terminal(payload).await
	}
}

pub(crate) struct GuardedView<O, E, P> {
	pub view: ErasedView<O, E, P>,
	pub context: StructuredContext,
}

impl<O: Sync, E: Sync, P: Send + 'static> View<O, E, P> for GuardedView<O, E, P> {
	type Completion = ();
	type Error = Failure;

	async fn progress(&mut self, event: P) -> Result<(), Failure> {
		self.view.progress(event).await
	}

	async fn finish(&mut self, outcome: &Result<O, E>) -> Result<(), Failure> {
		self.context.begin_finish();
		self.view.finish(outcome).await?;
		self.context.close(true)
	}
}

// Retained contexts cannot outlive a failed factory or an unwinding action.
pub(crate) struct CloseSink(StructuredContext);

impl Drop for CloseSink {
	fn drop(&mut self) {
		let _ = self.0.close(false);
	}
}
