//! Direct and CLI invocation parity, bounded event delivery and cancellation.

mod support;

use std::{num::NonZeroUsize, time::Duration};

use futures_lite::{future, pin};
use trine::{Action, Cancellation, EmitError, InvocationContext, cli::Parsed};

use support::workyard::{self, CreateInput, Failure};

fn capacity(value: usize) -> NonZeroUsize {
	NonZeroUsize::new(value).unwrap()
}

fn input() -> CreateInput {
	serde_json::from_value(serde_json::json!({
		"task": "feat/payments", "repositories": [{"path": "api"}, {"path": "web"}], "no_fetch": true,
	}))
	.unwrap()
}

#[test]
fn direct_and_cli_requests_reach_the_same_handler_and_result() {
	let action = workyard::action(Duration::ZERO);
	let cli = workyard::cli(action.clone());
	let Parsed::Input(parsed) = cli
		.parse([
			"demo",
			"create",
			"feat/payments",
			"--repo=api",
			"--repo=web",
			"--no-fetch",
		])
		.unwrap()
	else {
		panic!("expected action input");
	};
	assert_eq!(parsed, input());
	let direct =
		smol::block_on(action.invoke(input(), InvocationContext::unobserved(Cancellation::new())));
	let presented = cli.run(parsed, capacity(1), Cancellation::new(), |_| {});
	assert_eq!(direct, presented);
}

#[test]
fn cancellation_is_idempotent_and_wakes_all_waiters() {
	smol::block_on(async {
		let cancellation = Cancellation::new();
		assert!(!cancellation.is_cancelled());
		let first = cancellation.cancelled();
		let second = cancellation.cancelled();
		pin!(first, second);
		assert!(future::poll_once(&mut first).await.is_none());
		assert!(future::poll_once(&mut second).await.is_none());
		cancellation.cancel();
		cancellation.cancel();
		future::zip(first, second).await;
		assert!(cancellation.is_cancelled());
	});
}

#[test]
fn unobserved_events_are_noops_including_after_cancellation() {
	smol::block_on(async {
		let cancellation = Cancellation::new();
		let context = InvocationContext::unobserved(cancellation.clone());
		context.emit("first").await.unwrap();
		cancellation.cancel();
		context.emit("second").await.unwrap();
	});
}

#[test]
fn full_queue_waits_for_capacity_without_dropping_events() {
	smol::block_on(async {
		let (context, events) = InvocationContext::observed(capacity(1), Cancellation::new());
		context.emit(1).await.unwrap();
		let second = context.emit(2);
		pin!(second);
		assert!(future::poll_once(&mut second).await.is_none());
		assert_eq!(events.recv().await.unwrap(), 1);
		second.await.unwrap();
		assert_eq!(events.recv().await.unwrap(), 2);
	});
}

#[test]
fn cancellation_wakes_a_blocked_send_without_draining_or_timer() {
	smol::block_on(async {
		let cancellation = Cancellation::new();
		let (context, events) = InvocationContext::observed(capacity(1), cancellation.clone());
		context.emit(1).await.unwrap();
		let blocked = context.emit(2);
		pin!(blocked);
		assert!(future::poll_once(&mut blocked).await.is_none());
		cancellation.cancel();
		assert_eq!(blocked.await, Err(EmitError::Cancelled));
		assert_eq!(events.len(), 1);
		assert_eq!(events.recv().await.unwrap(), 1);
	});
}

#[test]
fn action_finishes_cooperatively_after_cancellation_while_queue_is_full() {
	let action: Action<(), bool, (), usize> =
		Action::new("run", "Exercise backpressure", |_, context| async move {
			context.emit(1).await.unwrap();
			assert_eq!(context.emit(2).await, Err(EmitError::Cancelled));
			Ok(true)
		})
		.unwrap();
	smol::block_on(async {
		let cancellation = Cancellation::new();
		let (context, events) = InvocationContext::observed(capacity(1), cancellation.clone());
		let operation = action.invoke((), context);
		pin!(operation);
		assert!(future::poll_once(&mut operation).await.is_none());
		cancellation.cancel();
		assert_eq!(operation.await, Ok(true));
		assert_eq!(events.recv().await.unwrap(), 1);
		assert!(events.recv().await.is_err());
	});
}

#[test]
fn completion_closes_retained_emitters_and_preserves_accepted_events() {
	let action: Action<(), InvocationContext<usize>, (), usize> = Action::new(
		"run",
		"Return a retained emitter",
		|_, context: InvocationContext<usize>| async move {
			context.emit(1).await.unwrap();
			context.emit(2).await.unwrap();
			Ok(context)
		},
	)
	.unwrap();
	smol::block_on(async {
		let (context, events) = InvocationContext::observed(capacity(2), Cancellation::new());
		let retained = action.invoke((), context).await.unwrap();
		assert_eq!(retained.emit(3).await, Err(EmitError::Closed));
		assert_eq!(events.recv().await.unwrap(), 1);
		assert_eq!(events.recv().await.unwrap(), 2);
		assert!(events.recv().await.is_err());
	});
}

#[test]
fn dropping_an_invocation_closes_its_stream() {
	let action: Action<(), (), (), ()> =
		Action::new("run", "Pending action", |_, _| future::pending()).unwrap();
	smol::block_on(async {
		let (context, events) = InvocationContext::observed(capacity(1), Cancellation::new());
		let mut operation = Box::pin(action.invoke((), context));
		assert!(future::poll_once(&mut operation).await.is_none());
		drop(operation);
		assert!(events.recv().await.is_err());
	});
}

#[test]
fn cancellation_preserves_completed_participants_and_does_not_complete_the_rest() {
	let action = workyard::action(Duration::ZERO);
	smol::block_on(async {
		let cancellation = Cancellation::new();
		let (context, events) =
			InvocationContext::<workyard::Progress>::observed(capacity(1), cancellation.clone());
		let consume = async {
			let mut completed = Vec::new();
			while let Ok(event) = events.recv().await {
				assert_eq!(event.repository, "api", "later participants must not start");
				if event.phase == "complete" {
					completed.push(event.repository);
					cancellation.cancel();
				}
			}
			completed
		};
		let (result, completed) = future::zip(action.invoke(input(), context), consume).await;
		let Err(Failure::Cancelled { outcome, .. }) = result else {
			panic!("expected cancelled partial result");
		};
		assert_eq!(completed, ["api"]);
		assert_eq!(outcome.completed, ["api"]);
		assert_eq!(outcome.remaining, ["web"]);
	});
}

#[test]
fn cli_drains_progress_before_returning_the_terminal_result() {
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	let mut rendered = Vec::new();
	let outcome = cli
		.run(input(), capacity(1), Cancellation::new(), |event| {
			rendered.push(format!("{}:{}", event.repository, event.phase))
		})
		.unwrap();
	rendered.push("terminal".into());
	assert_eq!(outcome.completed, ["api", "web"]);
	assert_eq!(
		rendered,
		[
			"api:create worktree",
			"api:complete",
			"web:create worktree",
			"web:complete",
			"terminal"
		]
	);
}

#[test]
fn shared_domain_validation_rejects_conflicting_flags_before_progress() {
	let action = workyard::action(Duration::ZERO);
	let cli = workyard::cli(action.clone());
	let Parsed::Input(parsed) = cli
		.parse([
			"demo",
			"create",
			"feature",
			"--repo=api",
			"--no-fetch",
			"--pull",
		])
		.unwrap()
	else {
		panic!("expected typed input");
	};
	let direct = smol::block_on(action.invoke(
		parsed.clone(),
		InvocationContext::unobserved(Cancellation::new()),
	));
	let presented = cli.run(parsed, capacity(1), Cancellation::new(), |_| {
		panic!("invalid request must emit no progress")
	});
	assert!(matches!(direct, Err(Failure::Invalid { .. })));
	assert_eq!(direct, presented);
}

#[test]
fn cancellation_wins_when_capacity_and_cancellation_are_both_ready() {
	smol::block_on(async {
		let cancellation = Cancellation::new();
		let (context, events) = InvocationContext::observed(capacity(1), cancellation.clone());
		context.emit(1).await.unwrap();
		let blocked = context.emit(2);
		pin!(blocked);
		assert!(future::poll_once(&mut blocked).await.is_none());

		// Make both branches ready before polling the blocked send again.
		cancellation.cancel();
		assert_eq!(events.recv().await.unwrap(), 1);
		assert_eq!(blocked.await, Err(EmitError::Cancelled));
		assert!(events.is_empty());
	});
}
