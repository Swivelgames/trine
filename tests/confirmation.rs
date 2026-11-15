use std::convert::Infallible;
use trine::{
	Action, Cancellation, InvocationContext,
	invocation::Confirmation,
	view::{self, View},
};

struct Answer(Option<bool>);

impl View<Option<bool>, (), ()> for Answer {
	type Completion = ();
	type Error = Infallible;

	async fn progress(&mut self, _: ()) -> Result<(), Infallible> {
		Ok(())
	}

	async fn confirm(
		&mut self,
		_: Confirmation,
		_: Cancellation,
	) -> Result<Option<bool>, Infallible> {
		Ok(self.0)
	}

	async fn finish(&mut self, _: &Result<Option<bool>, ()>) -> Result<(), Infallible> {
		Ok(())
	}
}

fn question() -> Confirmation {
	Confirmation {
		code: "confirm_test".into(),
		message: "Proceed?".into(),
	}
}

#[test]
fn unobserved_protocol_invocation_does_not_wait_for_input() {
	let context = InvocationContext::<()>::unobserved(Cancellation::new());
	assert_eq!(smol::block_on(context.confirm(question())).unwrap(), None);
}

#[test]
fn cli_view_can_answer_or_decline_interaction_without_progress_subscription() {
	for answer in [None, Some(false), Some(true)] {
		let action = Action::new(
			"ask",
			"ask",
			|_: (), context: InvocationContext<()>| async move {
				Ok::<_, ()>(context.confirm(question()).await.unwrap())
			},
		)
		.unwrap();
		let report = view::run(&action, (), Answer(answer), None, Cancellation::new());
		match report.outcome {
			trine::execution::ExecutionOutcome::Completed(Ok(actual)) => assert_eq!(actual, answer),
			_ => panic!("confirmation action did not finish"),
		}
	}
}

#[test]
fn cancellation_releases_a_pending_confirmation_view() {
	struct Waiting;
	impl View<(), (), ()> for Waiting {
		type Completion = ();
		type Error = Infallible;

		async fn progress(&mut self, _: ()) -> Result<(), Infallible> {
			Ok(())
		}

		async fn confirm(
			&mut self,
			_: Confirmation,
			cancellation: Cancellation,
		) -> Result<Option<bool>, Infallible> {
			cancellation.cancel();
			std::future::pending().await
		}

		async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), Infallible> {
			Ok(())
		}
	}
	let action = Action::new(
		"ask",
		"ask",
		|_: (), context: InvocationContext<()>| async move {
			assert_eq!(
				context.confirm(question()).await.unwrap_err(),
				trine::EmitError::Cancelled
			);
			Ok::<_, ()>(())
		},
	)
	.unwrap();
	let report = view::run(&action, (), Waiting, None, Cancellation::new());
	assert!(matches!(
		report.outcome,
		trine::execution::ExecutionOutcome::Completed(Ok(()))
	));
}

#[test]
fn all_accepted_disclosures_reach_the_view_before_confirmation() {
	use std::sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	};

	struct OrderedView {
		ready: Arc<AtomicBool>,
		seen: Vec<u8>,
	}

	impl View<(), (), u8> for OrderedView {
		type Completion = ();
		type Error = Infallible;

		async fn progress(&mut self, event: u8) -> Result<(), Infallible> {
			while !self.ready.load(Ordering::SeqCst) {
				futures_lite::future::yield_now().await;
			}
			self.seen.push(event);
			Ok(())
		}

		async fn confirm(
			&mut self,
			_: Confirmation,
			_: Cancellation,
		) -> Result<Option<bool>, Infallible> {
			assert_eq!(self.seen, [1, 2, 3]);
			Ok(Some(true))
		}

		async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), Infallible> {
			Ok(())
		}
	}

	let ready = Arc::new(AtomicBool::new(false));
	let action_ready = ready.clone();
	let action = Action::new(
		"ask",
		"ask",
		move |_: (), context: InvocationContext<u8>| {
			let ready = action_ready.clone();
			async move {
				for event in [1, 2, 3] {
					context.emit(event).await.unwrap();
				}

				ready.store(true, Ordering::SeqCst);
				assert_eq!(context.confirm(question()).await.unwrap(), Some(true));
				Ok::<_, ()>(())
			}
		},
	)
	.unwrap();
	let view = OrderedView {
		ready,
		seen: Vec::new(),
	};
	let report = view::run(
		&action,
		(),
		view,
		std::num::NonZeroUsize::new(3),
		Cancellation::new(),
	);
	assert!(matches!(
		report.outcome,
		trine::execution::ExecutionOutcome::Completed(Ok(()))
	));
}
