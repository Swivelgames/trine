//! One typed view per invocation, driven independently of the action.

mod decision;
mod erased;

pub(crate) use erased::ErasedView;

use crate::{
	Action, Cancellation, InvocationContext,
	execution::{ExecutionFailure, ExecutionOutcome, PresentationState},
};
use std::{
	future::Future,
	num::NonZeroUsize,
	panic::{AssertUnwindSafe, catch_unwind},
	sync::Arc,
};

/// Presents one invocation's ordered progress and terminal domain result. The factory creates a
/// fresh view for each invocation.
pub trait View<Output, Error, Event>: Send {
	/// Error returned when presentation or interaction fails.
	type Error: Send;
	/// Value returned after successful terminal presentation.
	type Completion: Send;

	/// Present an invocation's opening output before its handler starts.
	/// The default produces no output. An error prevents the handler from running.
	fn start(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
		async { Ok(()) }
	}

	/// Answer a yes-or-no request. The default declines to supply an answer; interactive
	/// implementations can monitor cancellation.
	fn confirm(
		&mut self,
		_request: crate::invocation::Confirmation,
		_cancellation: Cancellation,
	) -> impl Future<Output = Result<Option<bool>, Self::Error>> + Send {
		async { Ok(None) }
	}

	/// Return the zero-based index of a selected option, or `None` when interaction is
	/// unavailable.
	fn select(
		&mut self,
		_request: crate::invocation::Selection,
		_cancellation: Cancellation,
	) -> impl Future<Output = Result<Option<usize>, Self::Error>> + Send {
		async { Ok(None) }
	}

	/// Present the next accepted progress event in delivery order. The default ignores it.
	fn progress(&mut self, _event: Event) -> impl Future<Output = Result<(), Self::Error>> + Send {
		async { Ok(()) }
	}

	/// Present the returned domain result and produce a completion value. A handler panic
	/// leaves this callback uncalled.
	fn finish(
		&mut self,
		result: &Result<Output, Error>,
	) -> impl Future<Output = Result<Self::Completion, Self::Error>> + Send;
}

pub use crate::execution::{ExecutionReport as Report, PresentationFailure as PresentationError};

/// The worker is joined. An arbitrary blocking writer can still delay shutdown;
/// cancellation never forcibly drops an action or kills a rendering thread.
pub fn run<Input, Output, Error, Event, V>(
	action: &Action<Input, Output, Error, Event>,
	input: Input,
	view: V,
	capacity: Option<NonZeroUsize>,
	cancellation: Cancellation,
) -> Report<Output, Error, V::Error, V::Completion>
where
	Output: Send + Sync,
	Error: Send + Sync,
	Event: Send,
	V: View<Output, Error, Event>,
{
	let view = match smol::block_on(crate::execution::start_view(view)) {
		Ok(view) => view,
		Err(presentation) => return Report::startup_failed(presentation),
	};

	let (context, events) = match capacity {
		Some(capacity) => {
			let (context, events) = InvocationContext::observed(capacity, cancellation.clone());
			(context, Some(events))
		}
		None => (InvocationContext::unobserved(cancellation.clone()), None),
	};

	let (context, decisions) = context.with_decisions();
	std::thread::scope(|scope| {
		let (terminal_tx, terminal_rx) =
			async_channel::bounded::<Arc<ExecutionOutcome<Output, Error>>>(1);
		let worker =
			scope.spawn(move || render_worker(view, events, decisions, terminal_rx, cancellation));

		// Keep the result here even if a failed consumer has already exited.
		let result = catch_unwind(AssertUnwindSafe(|| {
			smol::block_on(action.invoke(input, context))
		}));
		let result = Arc::new(match result {
			Ok(result) => ExecutionOutcome::Completed(result),
			Err(_) => ExecutionOutcome::Failed(ExecutionFailure {
				message: "handler panicked; effects may be incomplete".into(),
			}),
		});

		let _ = terminal_tx.try_send(result.clone());
		drop(terminal_tx);

		let presentation = worker
			.join()
			.unwrap_or(PresentationState::Failed(PresentationError::Panicked));
		let outcome = Arc::try_unwrap(result)
			.unwrap_or_else(|_| unreachable!("joined view released terminal result"));

		Report {
			outcome,
			presentation,
		}
	})
}

/// Run blocking presentation on the joined worker, keeping its unwind boundary
/// separate from action execution on the caller's thread.
fn render_worker<O, E, P, V>(
	mut view: V,
	events: Option<async_channel::Receiver<P>>,
	decisions: crate::invocation::Requests,
	terminal_rx: async_channel::Receiver<Arc<ExecutionOutcome<O, E>>>,
	cancellation: Cancellation,
) -> PresentationState<V::Completion, PresentationError<V::Error>>
where
	O: Send + Sync,
	E: Send + Sync,
	P: Send,
	V: View<O, E, P>,
{
	let rendered = catch_unwind(AssertUnwindSafe(|| {
		smol::block_on(async {
			let mut progress_open = events.is_some();
			let mut decisions_open = true;

			while progress_open || decisions_open {
				let progress = async {
					if !progress_open {
						return std::future::pending().await;
					}
					Delivery::Progress(
						events
							.as_ref()
							.expect("progress receiver")
							.recv()
							.await
							.ok(),
					)
				};

				let decision = async {
					if !decisions_open {
						return std::future::pending().await;
					}
					Delivery::Decision(decisions.recv().await.ok())
				};

				let result = match futures_lite::future::or(progress, decision).await {
					Delivery::Progress(Some(event)) => view.progress(event).await,
					Delivery::Progress(None) => {
						progress_open = false;
						continue;
					}
					Delivery::Decision(Some(request)) => {
						decision::deliver(&mut view, events.as_ref(), request, &cancellation).await
					}
					Delivery::Decision(None) => {
						decisions_open = false;
						continue;
					}
				};

				if let Err(error) = result {
					if let Some(events) = &events {
						events.close();
					}
					decisions.close();
					cancellation.cancel();
					return PresentationState::Failed(PresentationError::Output(error));
				}
			}

			let Ok(result) = terminal_rx.recv().await else {
				return PresentationState::Skipped;
			};

			match result.as_ref() {
				ExecutionOutcome::Completed(result) => PresentationState::from(
					view.finish(result).await.map_err(PresentationError::Output),
				),
				ExecutionOutcome::Failed(_) | ExecutionOutcome::NotStarted => {
					PresentationState::Skipped
				}
			}
		})
	}));

	match rendered {
		Ok(result) => result,
		Err(_) => {
			if let Some(events) = &events {
				events.close();
			}
			cancellation.cancel();
			PresentationState::Failed(PresentationError::Panicked)
		}
	}
}

enum Delivery<P> {
	Progress(Option<P>),
	Decision(Option<crate::invocation::Request>),
}
