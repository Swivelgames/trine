use std::{
	num::NonZeroUsize,
	sync::{Arc, Mutex, mpsc},
};
use trine::{
	Action, Cancellation, InvocationContext,
	view::{self, PresentationError, View},
};

struct Recording {
	seen: Arc<Mutex<Vec<String>>>,
	fail_progress: bool,
	fail_finish: bool,
	panic_progress: bool,
}
impl View<usize, String, usize> for Recording {
	type Completion = ();
	type Error = &'static str;
	async fn progress(&mut self, event: usize) -> Result<(), Self::Error> {
		assert!(!self.panic_progress, "renderer panic");
		if self.fail_progress {
			return Err("broken output");
		}
		self.seen.lock().unwrap().push(event.to_string());
		Ok(())
	}
	async fn finish(&mut self, result: &Result<usize, String>) -> Result<(), Self::Error> {
		self.seen.lock().unwrap().push(format!("finish:{result:?}"));
		if self.fail_finish {
			return Err("terminal failure");
		}
		Ok(())
	}
}
fn recording() -> Recording {
	Recording {
		seen: Arc::default(),
		fail_progress: false,
		fail_finish: false,
		panic_progress: false,
	}
}
fn action() -> Action<(), usize, String, usize> {
	Action::new(
		"run",
		"Emit events",
		|_, context: InvocationContext<usize>| async move {
			for event in 0..3 {
				if context.emit(event).await.is_err() {
					return Err("cancelled and settled".into());
				}
			}
			Ok(3)
		},
	)
	.unwrap()
}
#[test]
fn drains_fifo_then_finishes_once() {
	let renderer = recording();
	let seen = renderer.seen.clone();
	let report = view::run(
		&action(),
		(),
		renderer,
		NonZeroUsize::new(1),
		Cancellation::new(),
	);
	assert_eq!(report.outcome, Ok(3));
	assert!(report.presentation.is_ok());
	assert_eq!(*seen.lock().unwrap(), ["0", "1", "2", "finish:Ok(3)"]);
}
#[test]
fn output_failure_cancels_and_waits_for_settlement() {
	let mut renderer = recording();
	renderer.fail_progress = true;
	let seen = renderer.seen.clone();
	let cancellation = Cancellation::new();
	let action: Action<(), usize, String, usize> = Action::new(
		"run",
		"Settle after cancellation",
		|_, context: InvocationContext<usize>| async move {
			let _ = context.emit(1).await;
			context.cancellation().cancelled().await;
			Ok(42)
		},
	)
	.unwrap();
	let report = view::run(
		&action,
		(),
		renderer,
		NonZeroUsize::new(1),
		cancellation.clone(),
	);
	assert!(cancellation.is_cancelled());
	assert_eq!(report.outcome, Ok(42));
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Failed(PresentationError::Output("broken output"))
	));
	assert!(seen.lock().unwrap().is_empty());
}
#[test]
fn finish_failure_keeps_domain_result() {
	let mut renderer = recording();
	renderer.fail_finish = true;
	let report = view::run(
		&action(),
		(),
		renderer,
		NonZeroUsize::new(1),
		Cancellation::new(),
	);
	assert_eq!(report.outcome, Ok(3));
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Failed(PresentationError::Output("terminal failure"))
	));
}
#[test]
fn unsubscribed_view_receives_only_terminal() {
	let renderer = recording();
	let seen = renderer.seen.clone();
	let report = view::run(&action(), (), renderer, None, Cancellation::new());
	assert_eq!(report.outcome, Ok(3));
	assert_eq!(*seen.lock().unwrap(), ["finish:Ok(3)"]);
}
#[test]
fn renderer_panic_cancels_without_deadlocking_full_queue() {
	let mut renderer = recording();
	renderer.panic_progress = true;
	let report = view::run(
		&action(),
		(),
		renderer,
		NonZeroUsize::new(1),
		Cancellation::new(),
	);
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Failed(PresentationError::Panicked)
	));
}
#[test]
fn action_panic_releases_waiting_view() {
	let action: Action<(), usize, String, usize> =
		Action::new("run", "Panic", |_, _| async { panic!("action panic") }).unwrap();
	let report = view::run(
		&action,
		(),
		recording(),
		NonZeroUsize::new(1),
		Cancellation::new(),
	);
	assert!(matches!(
		report.outcome,
		trine::execution::ExecutionOutcome::Failed(_)
	));
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Skipped
	));
}
struct Blocking {
	unblock_action: async_channel::Sender<()>,
	entered: mpsc::Sender<()>,
	release: mpsc::Receiver<()>,
	count: usize,
}
impl View<usize, String, usize> for Blocking {
	type Completion = ();
	type Error = ();
	async fn progress(&mut self, _: usize) -> Result<(), ()> {
		if self.count == 0 {
			self.entered.send(()).unwrap();
			self.unblock_action.try_send(()).unwrap();
			self.release.recv().unwrap();
		}
		self.count += 1;
		Ok(())
	}
	async fn finish(&mut self, _: &Result<usize, String>) -> Result<(), ()> {
		assert_eq!(self.count, 3);
		Ok(())
	}
}
#[test]
fn blocked_view_does_not_occupy_action_thread() {
	let (entered_tx, entered_rx) = mpsc::channel();
	let (release_tx, release_rx) = mpsc::channel();
	let (continued_tx, continued_rx) = mpsc::channel();
	let (unblock_action, view_started) = async_channel::bounded(1);
	let action: Action<(), usize, String, usize> = Action::new(
		"run",
		"Continue during blocking view",
		move |_, context: InvocationContext<usize>| {
			let continued = continued_tx.clone();
			let view_started = view_started.clone();
			async move {
				context.emit(0).await.unwrap();
				view_started.recv().await.unwrap();
				context.emit(1).await.unwrap();
				context.emit(2).await.unwrap();
				continued.send(()).unwrap();
				Ok(3)
			}
		},
	)
	.unwrap();
	std::thread::scope(|scope| {
		let run = scope.spawn(|| {
			view::run(
				&action,
				(),
				Blocking {
					unblock_action,
					entered: entered_tx,
					release: release_rx,
					count: 0,
				},
				NonZeroUsize::new(4),
				Cancellation::new(),
			)
		});
		entered_rx
			.recv_timeout(std::time::Duration::from_secs(5))
			.unwrap();
		let continued = continued_rx.recv_timeout(std::time::Duration::from_secs(5));
		release_tx.send(()).unwrap();
		assert!(continued.is_ok());
		assert_eq!(run.join().unwrap().outcome, Ok(3));
	});
}
