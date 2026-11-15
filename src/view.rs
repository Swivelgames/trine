//! One typed view per invocation, driven independently of the action.

mod confirmation;
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

pub trait View<Output, Error, Event>: Send {
	type Error: Send;
	type Completion: Send;

	fn confirm(
		&mut self,
		_request: crate::invocation::Confirmation,
		_cancellation: Cancellation,
	) -> impl Future<Output = Result<Option<bool>, Self::Error>> + Send {
		async { Ok(None) }
	}

	fn progress(&mut self, event: Event) -> impl Future<Output = Result<(), Self::Error>> + Send;

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
	let (context, events) = match capacity {
		Some(capacity) => {
			let (context, events) = InvocationContext::observed(capacity, cancellation.clone());
			(context, Some(events))
		}
		None => (InvocationContext::unobserved(cancellation.clone()), None),
	};

	let (context, confirmations) = context.with_confirmations();
	std::thread::scope(|scope| {
		let (terminal_tx, terminal_rx) =
			async_channel::bounded::<Arc<ExecutionOutcome<Output, Error>>>(1);
		let worker = scope
			.spawn(move || render_worker(view, events, confirmations, terminal_rx, cancellation));

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
	confirmations: crate::invocation::Requests,
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
			let mut confirmations_open = true;
			while progress_open || confirmations_open {
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
				let confirmation = async {
					if !confirmations_open {
						return std::future::pending().await;
					}
					Delivery::Confirmation(confirmations.recv().await.ok())
				};
				let result = match futures_lite::future::or(progress, confirmation).await {
					Delivery::Progress(Some(event)) => view.progress(event).await,
					Delivery::Progress(None) => {
						progress_open = false;
						continue;
					}
					Delivery::Confirmation(Some(request)) => {
						confirmation::deliver(&mut view, events.as_ref(), request, &cancellation)
							.await
					}
					Delivery::Confirmation(None) => {
						confirmations_open = false;
						continue;
					}
				};
				if let Err(error) = result {
					if let Some(events) = &events {
						events.close();
					}
					confirmations.close();
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
				ExecutionOutcome::Failed(_) => PresentationState::Skipped,
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
	Confirmation(Option<crate::invocation::Request>),
}
