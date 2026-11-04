//! One typed view per invocation, driven independently of the action.

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

	std::thread::scope(|scope| {
		let (terminal_tx, terminal_rx) =
			async_channel::bounded::<Arc<ExecutionOutcome<Output, Error>>>(1);
		let worker = scope.spawn(move || render_worker(view, events, terminal_rx, cancellation));

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
			if let Some(events) = &events {
				while let Ok(event) = events.recv().await {
					if let Err(error) = view.progress(event).await {
						events.close();
						cancellation.cancel();
						return PresentationState::Failed(PresentationError::Output(error));
					}
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
