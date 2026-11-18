//! Invocation-scoped cooperative cancellation and optional bounded progress.

mod decision;
pub use decision::{Confirmation, Selection};
pub(crate) use decision::{Request, Requests};

use std::{fmt, num::NonZeroUsize};

use async_channel::{Receiver, Sender};
use futures_lite::future;

/// A clonable cancellation request. Cancellation is idempotent and broadcasts to
/// all waiters; it never drops a handler or rolls back domain work.
#[derive(Clone, Debug)]
pub struct Cancellation {
	signal: Receiver<()>,
	_sender: Sender<()>,
}

impl Default for Cancellation {
	fn default() -> Self {
		Self::new()
	}
}

impl Cancellation {
	pub fn new() -> Self {
		let (sender, signal) = async_channel::bounded(1);
		Self {
			signal,
			_sender: sender,
		}
	}

	pub fn cancel(&self) {
		self.signal.close();
	}

	pub fn is_cancelled(&self) -> bool {
		self.signal.is_closed()
	}

	pub async fn cancelled(&self) {
		let _ = self.signal.recv().await;
	}
}

/// Why a subscribed progress event could not be accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitError {
	Cancelled,
	Closed,
}

impl fmt::Display for EmitError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Cancelled => "invocation cancelled",
			Self::Closed => "progress stream closed",
		})
	}
}

impl std::error::Error for EmitError {}

/// One context per invocation. Clones share cancellation and emitter closure.
/// Events are optional observations, not the action's terminal result.
#[derive(Debug)]
pub struct InvocationContext<Event> {
	cancellation: Cancellation,
	events: Option<Sender<Event>>,
	decisions: Option<Sender<Request>>,
}

impl<Event> Clone for InvocationContext<Event> {
	fn clone(&self) -> Self {
		Self {
			cancellation: self.cancellation.clone(),
			events: self.events.clone(),
			decisions: self.decisions.clone(),
		}
	}
}

impl<Event> InvocationContext<Event> {
	pub fn unobserved(cancellation: Cancellation) -> Self {
		Self {
			cancellation,
			events: None,
			decisions: None,
		}
	}

	pub fn observed(capacity: NonZeroUsize, cancellation: Cancellation) -> (Self, Receiver<Event>) {
		let (events, receiver) = async_channel::bounded(capacity.get());
		(
			Self {
				cancellation,
				events: Some(events),
				decisions: None,
			},
			receiver,
		)
	}

	pub(crate) fn with_decisions(mut self) -> (Self, Requests) {
		let (sender, receiver) = async_channel::bounded(1);
		self.decisions = Some(sender);
		(self, receiver)
	}

	/// Ask the selected view. Unsubscribed interfaces return None immediately.
	pub async fn confirm(&self, confirmation: Confirmation) -> Result<Option<bool>, EmitError> {
		let Some(requests) = &self.decisions else {
			return Ok(None);
		};
		decision::ask(
			requests,
			|reply| Request::Confirm(confirmation, reply),
			&self.cancellation,
		)
		.await
	}

	/// Ask the selected view for a zero-based option. A noninteractive view returns None.
	pub async fn select(&self, selection: Selection) -> Result<Option<usize>, EmitError> {
		let Some(requests) = &self.decisions else {
			return Ok(None);
		};
		decision::ask(
			requests,
			|reply| Request::Select(selection, reply),
			&self.cancellation,
		)
		.await
	}

	pub fn cancellation(&self) -> &Cancellation {
		&self.cancellation
	}

	/// Unobserved emission is a no-op, even after cancellation. Handlers check
	/// cancellation separately at domain-safe boundaries.
	pub async fn emit(&self, event: Event) -> Result<(), EmitError> {
		let Some(events) = &self.events else {
			return Ok(());
		};
		if self.cancellation.is_cancelled() {
			return Err(EmitError::Cancelled);
		}

		future::or(
			async {
				self.cancellation.cancelled().await;
				Err(EmitError::Cancelled)
			},
			async { events.send(event).await.map_err(|_| EmitError::Closed) },
		)
		.await
	}

	/// Stop accepting events; already accepted events remain available to drain.
	/// Action completion calls this even if a handler retains a context clone.
	pub fn close_events(&self) {
		if let Some(decisions) = &self.decisions {
			decisions.close();
		}
		if let Some(events) = &self.events {
			events.close();
		}
	}
}
