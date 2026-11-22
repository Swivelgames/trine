//! Executor-neutral execution. Hosts must retain and await the execution future.

use crate::{Action, Cancellation, InvocationContext, view::View};
use std::{
	any::Any,
	future::{Future, poll_fn},
	num::NonZeroUsize,
	panic::{AssertUnwindSafe, catch_unwind},
	sync::Arc,
};

/// Abnormal handler termination before a domain result was available.
#[derive(Clone, Debug)]
pub struct ExecutionFailure {
	/// Diagnostic describing the abnormal termination.
	pub message: String,
}

/// Settlement of the handler, tracked independently of the view.
#[derive(Debug)]
pub enum ExecutionOutcome<O, E> {
	/// The handler returned its domain result, including any domain error.
	Completed(Result<O, E>),
	/// Execution terminated abnormally.
	Failed(ExecutionFailure),
}

/// Failure encountered while presenting an invocation.
#[derive(Debug)]
pub enum PresentationFailure<E> {
	/// The view returned its own error.
	Output(E),
	/// The view panicked during presentation or cleanup.
	Panicked,
}

/// The handler result and the separately observed terminal state of its view.
#[derive(Debug)]
pub struct ExecutionReport<O, E, V, C = ()> {
	/// Handler settlement, available even when presentation failed.
	pub outcome: ExecutionOutcome<O, E>,
	/// View settlement, including its completion value when successful.
	pub presentation: PresentationState<C, PresentationFailure<V>>,
}

/// Whether the view finished, failed, or had no domain result to present.
#[derive(Debug)]
pub enum PresentationState<C, E> {
	/// The view finished and supplied its completion value.
	Completed(C),
	/// The view failed to present the invocation.
	Failed(E),
	/// No domain result was available for terminal presentation.
	Skipped,
}

impl<C, E> PresentationState<C, E> {
	/// Return true when terminal presentation completed.
	pub fn is_ok(&self) -> bool {
		matches!(self, Self::Completed(_))
	}

	/// Return true when presentation failed. Skipped presentation yields false.
	pub fn is_err(&self) -> bool {
		matches!(self, Self::Failed(_))
	}
}

impl<C, E> From<Result<C, E>> for PresentationState<C, E> {
	fn from(result: Result<C, E>) -> Self {
		match result {
			Ok(value) => Self::Completed(value),
			Err(error) => Self::Failed(error),
		}
	}
}

/// Catch unwinding on each poll without selecting an executor or spawning work.
pub(crate) async fn supervise<F: Future>(future: F) -> Result<F::Output, Box<dyn Any + Send>> {
	// Own the pinned future so its destructor runs inside a separate unwind
	// boundary after polling has completed or unwound.
	let mut future = Box::pin(future);
	let result =
		poll_fn(
			|context| match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
				Ok(poll) => poll.map(Ok),
				Err(payload) => std::task::Poll::Ready(Err(payload)),
			},
		)
		.await;

	match catch_unwind(AssertUnwindSafe(|| drop(future))) {
		Ok(()) => result,
		Err(payload) => Err(payload),
	}
}

/// Drive the handler and view concurrently on the current executor. Progress uses the supplied
/// bounded capacity; a view failure requests cooperative cancellation. Await the future through
/// settlement to retain the report.
pub async fn run<I, O, E, P, V>(
	action: &Action<I, O, E, P>,
	input: I,
	mut view: V,
	capacity: Option<NonZeroUsize>,
	cancellation: Cancellation,
) -> ExecutionReport<O, E, V::Error, V::Completion>
where
	O: Send + Sync,
	E: Send + Sync,
	P: Send,
	V: View<O, E, P>,
{
	let (context, events) = match capacity {
		Some(capacity) => {
			let (context, events) = InvocationContext::observed(capacity, cancellation.clone());
			(context, Some(events))
		}
		None => (InvocationContext::unobserved(cancellation.clone()), None),
	};

	let close_context = context.clone();
	let (terminal_tx, terminal_rx) = async_channel::bounded(1);

	let execute = async {
		let outcome = match supervise(async { action.invoke(input, context).await }).await {
			Ok(outcome) => ExecutionOutcome::Completed(outcome),
			Err(payload) => ExecutionOutcome::Failed(ExecutionFailure {
				message: payload
					.downcast_ref::<String>()
					.cloned()
					.or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
					.unwrap_or_else(|| "handler panicked with a non-text payload".into()),
			}),
		};

		close_context.close_events();
		let outcome = Arc::new(outcome);
		let _ = terminal_tx.try_send(outcome.clone());
		drop(terminal_tx);
		outcome
	};

	let present = async {
		let result = supervise(async {
			if let Some(events) = &events {
				while let Ok(event) = events.recv().await {
					view.progress(event).await?;
				}
			}

			if let Ok(outcome) = terminal_rx.recv().await
				&& let ExecutionOutcome::Completed(result) = outcome.as_ref()
			{
				return view.finish(result).await.map(Some);
			}
			Ok(None)
		})
		.await;

		let cleanup = catch_unwind(AssertUnwindSafe(|| drop(view)));
		let result = match (result, cleanup) {
			(Ok(Ok(Some(value))), Ok(())) => PresentationState::Completed(value),
			(Ok(Ok(None)), Ok(())) => PresentationState::Skipped,
			(Ok(Err(error)), Ok(())) => {
				PresentationState::Failed(PresentationFailure::Output(error))
			}
			_ => PresentationState::Failed(PresentationFailure::Panicked),
		};

		if result.is_err() {
			if let Some(events) = &events {
				events.close();
			}
			cancellation.cancel();
		}
		result
	};

	let (outcome, presentation) = futures_lite::future::zip(execute, present).await;
	drop(terminal_rx);
	let outcome =
		Arc::try_unwrap(outcome).unwrap_or_else(|_| unreachable!("presentation released outcome"));

	ExecutionReport {
		outcome,
		presentation,
	}
}

/// A typed action/view pair ready for host scheduling. Construct the view before
/// creating this value; neither construction nor cancellation invokes the action.
#[must_use = "retain the invocation and await run() to observe settlement"]
pub struct PreparedInvocation<I, O, E, P, V> {
	action: Action<I, O, E, P>,
	input: I,
	view: V,
	capacity: Option<NonZeroUsize>,
	cancellation: Cancellation,
}

impl<I, O, E, P, V> PreparedInvocation<I, O, E, P, V> {
	/// Bind an action, input and view for later execution with the supplied progress capacity
	/// and cancellation token.
	pub fn new(
		action: Action<I, O, E, P>,
		input: I,
		view: V,
		capacity: Option<NonZeroUsize>,
		cancellation: Cancellation,
	) -> Self {
		Self {
			action,
			input,
			view,
			capacity,
			cancellation,
		}
	}

	/// Clone the invocation's cooperative cancellation handle.
	pub fn cancellation(&self) -> Cancellation {
		self.cancellation.clone()
	}
}

impl<I: Send, O: Send + Sync, E: Send + Sync, P: Send, V: View<O, E, P>>
	PreparedInvocation<I, O, E, P, V>
{
	/// Consume the prepared invocation and await both handler and view settlement.
	pub async fn run(self) -> ExecutionReport<O, E, V::Error, V::Completion> {
		run(
			&self.action,
			self.input,
			self.view,
			self.capacity,
			self.cancellation,
		)
		.await
	}
}

impl<O, E> ExecutionOutcome<O, E> {
	/// Return true only for a normally returned successful domain result.
	pub fn is_ok(&self) -> bool {
		matches!(self, Self::Completed(Ok(_)))
	}
}

impl<O: std::fmt::Debug, E: std::fmt::Debug> ExecutionOutcome<O, E> {
	/// Return the successful domain value.
	///
	/// # Panics
	/// Panics on a domain error or abnormal execution.
	pub fn unwrap(self) -> O {
		match self {
			Self::Completed(result) => result.unwrap(),
			Self::Failed(failure) => panic!("execution failed: {}", failure.message),
		}
	}
}

impl<O: PartialEq, E: PartialEq> PartialEq<Result<O, E>> for ExecutionOutcome<O, E> {
	fn eq(&self, other: &Result<O, E>) -> bool {
		matches!(self, Self::Completed(result) if result == other)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	struct PanicOnPollAndDrop;

	impl Future for PanicOnPollAndDrop {
		type Output = ();

		fn poll(
			self: std::pin::Pin<&mut Self>,
			_: &mut std::task::Context<'_>,
		) -> std::task::Poll<()> {
			panic!("poll failed")
		}
	}

	impl Drop for PanicOnPollAndDrop {
		fn drop(&mut self) {
			panic!("future cleanup failed")
		}
	}

	#[test]
	fn future_cleanup_after_caught_poll_panic_is_supervised() {
		smol::block_on(async {
			let settled = std::sync::atomic::AtomicBool::new(false);
			let companion = async {
				settled.store(true, std::sync::atomic::Ordering::SeqCst);
			};
			let (result, ()) =
				futures_lite::future::zip(supervise(PanicOnPollAndDrop), companion).await;
			assert!(result.is_err());
			assert!(settled.load(std::sync::atomic::Ordering::SeqCst));
		});
	}
}
