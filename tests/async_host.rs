use std::{
	num::NonZeroUsize,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Cancellation, InvocationContext,
	execution::{ExecutionOutcome, PreparedInvocation, PresentationFailure},
	view::View,
};

struct GateView {
	entered: async_channel::Sender<()>,
	release: async_channel::Receiver<()>,
	finishes: Arc<AtomicUsize>,
}
impl View<usize, (), usize> for GateView {
	type Completion = ();
	type Error = ();
	async fn progress(&mut self, _: usize) -> Result<(), ()> {
		self.entered.send(()).await.unwrap();
		self.release.recv().await.unwrap();
		Ok(())
	}
	async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), ()> {
		self.finishes.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}
}

#[test]
fn saturated_progress_does_not_block_host_cancellation() {
	smol::block_on(async {
		let (entered_tx, entered_rx) = async_channel::bounded(1);
		let (release_tx, release_rx) = async_channel::bounded(1);
		let (full_tx, full_rx) = async_channel::bounded(1);
		let finishes = Arc::new(AtomicUsize::new(0));
		let action = Action::new(
			"test",
			"test",
			move |_: (), context: InvocationContext<usize>| {
				let full_tx = full_tx.clone();
				async move {
					context.emit(1).await.unwrap();
					context.emit(2).await.unwrap();
					full_tx.send(()).await.unwrap();
					assert!(context.emit(3).await.is_err());
					Ok::<_, ()>(2)
				}
			},
		)
		.unwrap();
		let prepared = PreparedInvocation::new(
			action,
			(),
			GateView {
				entered: entered_tx,
				release: release_rx,
				finishes: finishes.clone(),
			},
			NonZeroUsize::new(1),
			Cancellation::new(),
		);
		let cancellation = prepared.cancellation();
		let control = async {
			entered_rx.recv().await.unwrap();
			full_rx.recv().await.unwrap();
			cancellation.cancel();
			release_tx.send(()).await.unwrap();
			entered_rx.recv().await.unwrap();
			release_tx.send(()).await.unwrap();
		};
		let (report, ()) = futures_lite::future::zip(prepared.run(), control).await;
		assert!(matches!(report.outcome, ExecutionOutcome::Completed(Ok(2))));
		assert!(report.presentation.is_ok());
		assert_eq!(finishes.load(Ordering::SeqCst), 1);
	});
}

struct PanicView;
impl View<(), (), ()> for PanicView {
	type Completion = ();
	type Error = ();
	async fn progress(&mut self, _: ()) -> Result<(), ()> {
		panic!("view failed")
	}
	async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), ()> {
		panic!("finish must not run")
	}
}
#[test]
fn retains_simultaneous_handler_and_view_panics() {
	let action = Action::new(
		"test",
		"test",
		|_: (), context: InvocationContext<()>| async move {
			context.emit(()).await.unwrap();
			panic!("handler failed");
			#[allow(unreachable_code)]
			Ok::<(), ()>(())
		},
	)
	.unwrap();
	let report = smol::block_on(
		PreparedInvocation::new(
			action,
			(),
			PanicView,
			NonZeroUsize::new(1),
			Cancellation::new(),
		)
		.run(),
	);
	assert!(matches!(report.outcome, ExecutionOutcome::Failed(_)));
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Failed(PresentationFailure::Panicked)
	));
}

#[test]
fn classified_failure_retains_underlying_error() {
	use std::error::Error;
	use trine::error::{Failure, FailureKind};
	let error = Failure::new(
		FailureKind::OutputFailed,
		std::io::Error::new(std::io::ErrorKind::BrokenPipe, "closed output"),
	);
	assert_eq!(error.kind, FailureKind::OutputFailed);
	assert_eq!(
		error
			.source()
			.unwrap()
			.downcast_ref::<std::io::Error>()
			.unwrap()
			.kind(),
		std::io::ErrorKind::BrokenPipe
	);
}

struct QuietView(Arc<AtomicUsize>);
impl View<usize, (), ()> for QuietView {
	type Completion = ();
	type Error = ();
	async fn progress(&mut self, _: ()) -> Result<(), ()> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), ()> {
		self.0.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}
}
#[test]
fn preparation_does_not_run_and_late_cancellation_keeps_success() {
	let calls = Arc::new(AtomicUsize::new(0));
	let action_calls = calls.clone();
	let action = Action::new(
		"test",
		"test",
		move |_: (), context: InvocationContext<()>| {
			let calls = action_calls.clone();
			async move {
				calls.fetch_add(1, Ordering::SeqCst);
				context.cancellation().cancel();
				Ok::<_, ()>(7)
			}
		},
	)
	.unwrap();
	let finishes = Arc::new(AtomicUsize::new(0));
	let invocation = PreparedInvocation::new(
		action,
		(),
		QuietView(finishes.clone()),
		None,
		Cancellation::new(),
	);
	assert_eq!(calls.load(Ordering::SeqCst), 0);
	let report = smol::block_on(invocation.run());
	assert!(matches!(report.outcome, ExecutionOutcome::Completed(Ok(7))));
	assert_eq!(finishes.load(Ordering::SeqCst), 1);
}

#[test]
fn handler_panic_drains_progress_without_typed_finish() {
	smol::block_on(async {
		let (entered_tx, entered_rx) = async_channel::bounded(1);
		let (release_tx, release_rx) = async_channel::bounded(1);
		let finishes = Arc::new(AtomicUsize::new(0));
		let action = Action::new(
			"test",
			"test",
			|_: (), context: InvocationContext<usize>| async move {
				context.emit(1).await.unwrap();
				panic!("handler failed");
				#[allow(unreachable_code)]
				Ok::<usize, ()>(0)
			},
		)
		.unwrap();
		let prepared = PreparedInvocation::new(
			action,
			(),
			GateView {
				entered: entered_tx,
				release: release_rx,
				finishes: finishes.clone(),
			},
			NonZeroUsize::new(1),
			Cancellation::new(),
		);
		let consumer = async {
			entered_rx.recv().await.unwrap();
			release_tx.send(()).await.unwrap();
		};
		let (report, ()) = futures_lite::future::zip(prepared.run(), consumer).await;
		assert!(matches!(report.outcome, ExecutionOutcome::Failed(_)));
		assert!(matches!(
			report.presentation,
			trine::execution::PresentationState::Skipped
		));
		assert_eq!(finishes.load(Ordering::SeqCst), 0);
	});
}

#[test]
fn shared_preparation_and_framework_diagnostic_are_separate_from_domain_finish() {
	use trine::{
		Application, Entry,
		structured::{Payload, StructuredBinding},
	};
	let calls = Arc::new(AtomicUsize::new(0));
	let handler_calls = calls.clone();
	let action: Action<(), usize, (), ()> = Action::new("panic", "Panic", move |_, _| {
		let calls = handler_calls.clone();
		async move {
			calls.fetch_add(1, Ordering::SeqCst);
			panic!("failure")
		}
	})
	.unwrap();
	let mut app = Application::new("test", "1").unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::serde()))
		.unwrap();
	let prepared = app
		.prepare_rpc(
			&["panic"],
			serde_json::Value::Null,
			false,
			Cancellation::new(),
		)
		.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), 0);
	let report = smol::block_on(prepared.run());
	assert!(report.execution.execution_failure.is_some());
	assert!(matches!(
		report.transcript.terminal,
		Some(Payload::FrameworkFailure { .. })
	));
	assert_eq!(
		report.execution.presentation_status,
		trine::registry::PresentationStatus::Skipped
	);
}

struct BrokenOutput;
impl View<usize, (), ()> for BrokenOutput {
	type Completion = ();
	type Error = std::io::Error;
	async fn progress(&mut self, _: ()) -> Result<(), Self::Error> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), Self::Error> {
		Err(std::io::Error::new(
			std::io::ErrorKind::BrokenPipe,
			"output closed",
		))
	}
}
#[test]
fn shared_dispatch_preserves_output_cause_and_success() {
	use std::error::Error;
	use trine::{
		Application, Entry,
		error::{Failure, FailureKind},
		structured::StructuredBinding,
	};
	let action = Action::new("test", "Test", |_: (), _: InvocationContext<()>| async {
		Ok::<_, ()>(7)
	})
	.unwrap();
	let mut app = Application::new("test", "1").unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::custom(|_| Ok(BrokenOutput))))
		.unwrap();
	let report = smol::block_on(
		app.prepare_rpc(
			&["test"],
			serde_json::Value::Null,
			false,
			Cancellation::new(),
		)
		.unwrap()
		.run(),
	);
	assert_eq!(
		report.execution.presentation_failure(),
		Some(FailureKind::OutputFailed)
	);
	assert_eq!(
		report
			.execution
			.presentation_cause()
			.unwrap()
			.source()
			.unwrap()
			.downcast_ref::<std::io::Error>()
			.unwrap()
			.kind(),
		std::io::ErrorKind::BrokenPipe,
	);
	let typed = report.execution.typed::<usize, (), Failure>().unwrap();
	assert!(matches!(typed.outcome, ExecutionOutcome::Completed(Ok(7))));
	let trine::execution::PresentationState::Failed(PresentationFailure::Output(error)) =
		&typed.presentation
	else {
		panic!("missing output error")
	};
	assert_eq!(error.kind, FailureKind::OutputFailed);
	assert_eq!(
		error
			.source()
			.unwrap()
			.downcast_ref::<std::io::Error>()
			.unwrap()
			.kind(),
		std::io::ErrorKind::BrokenPipe
	);
}

struct Unencodable;
impl serde::Serialize for Unencodable {
	fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
		Err(serde::ser::Error::custom("cannot encode"))
	}
}
#[test]
fn serialization_failure_is_classified_without_losing_success() {
	use trine::{
		Application, Entry,
		error::{Failure, FailureKind},
		structured::StructuredBinding,
	};
	let action = Action::new("test", "Test", |_: (), _: InvocationContext<()>| async {
		Ok::<_, ()>(Unencodable)
	})
	.unwrap();
	let mut app = Application::new("test", "1").unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::serde()))
		.unwrap();
	let report = app
		.invoke_rpc(
			&["test"],
			serde_json::Value::Null,
			false,
			Cancellation::new(),
		)
		.unwrap();
	assert!(report.execution.domain_succeeded);
	let typed = report
		.execution
		.typed::<Unencodable, (), Failure>()
		.unwrap();
	let trine::execution::PresentationState::Failed(PresentationFailure::Output(error)) =
		&typed.presentation
	else {
		panic!("expected encoding failure")
	};
	assert_eq!(error.kind, FailureKind::EncodingFailed);
}

#[test]
fn factory_source_survives_preparation_failure() {
	use std::error::Error;
	use trine::{
		Application, Entry,
		error::{Failure, FailureKind},
		structured::StructuredBinding,
	};
	let action = Action::new("test", "Test", |_: (), _: InvocationContext<()>| async {
		Ok::<_, ()>(7)
	})
	.unwrap();
	let mut app = Application::new("test", "1").unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::custom(|_| {
		Err::<BrokenOutput, _>(Failure::new(
			FailureKind::ViewSetupFailed,
			std::io::Error::new(std::io::ErrorKind::PermissionDenied, "view unavailable"),
		))
	})))
	.unwrap();
	let Err(error) = app.prepare_rpc(
		&["test"],
		serde_json::Value::Null,
		false,
		Cancellation::new(),
	) else {
		panic!("expected setup failure")
	};
	assert_eq!(error.kind(), FailureKind::ViewSetupFailed);
	assert_eq!(
		error
			.source()
			.unwrap()
			.source()
			.unwrap()
			.downcast_ref::<std::io::Error>()
			.unwrap()
			.kind(),
		std::io::ErrorKind::PermissionDenied
	);
}

struct PanicOnViewDrop;
impl View<usize, (), ()> for PanicOnViewDrop {
	type Completion = ();
	type Error = ();
	async fn progress(&mut self, _: ()) -> Result<(), ()> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), ()> {
		Ok(())
	}
}
impl Drop for PanicOnViewDrop {
	fn drop(&mut self) {
		panic!("view cleanup failed")
	}
}
#[test]
fn view_cleanup_panic_preserves_domain_success() {
	let action = Action::new("test", "Test", |_: (), _: InvocationContext<()>| async {
		Ok::<_, ()>(91)
	})
	.unwrap();
	let report = smol::block_on(
		PreparedInvocation::new(action, (), PanicOnViewDrop, None, Cancellation::new()).run(),
	);
	assert!(matches!(
		report.outcome,
		ExecutionOutcome::Completed(Ok(91))
	));
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Failed(PresentationFailure::Panicked)
	));
}

struct PanicOnProgress;
impl View<usize, (), ()> for PanicOnProgress {
	type Completion = ();
	type Error = ();
	async fn progress(&mut self, _: ()) -> Result<(), ()> {
		panic!("view failed")
	}
	async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), ()> {
		panic!("finish must not run")
	}
}
#[test]
fn async_view_panic_cancels_and_settles_live_handler() {
	let action = Action::new(
		"test",
		"Test",
		|_: (), context: InvocationContext<()>| async move {
			context.emit(()).await.unwrap();
			context.cancellation().cancelled().await;
			Ok::<_, ()>(73)
		},
	)
	.unwrap();
	let report = smol::block_on(
		PreparedInvocation::new(
			action,
			(),
			PanicOnProgress,
			NonZeroUsize::new(1),
			Cancellation::new(),
		)
		.run(),
	);
	assert!(matches!(
		report.outcome,
		ExecutionOutcome::Completed(Ok(73))
	));
	assert!(matches!(
		report.presentation,
		trine::execution::PresentationState::Failed(PresentationFailure::Panicked)
	));
}
