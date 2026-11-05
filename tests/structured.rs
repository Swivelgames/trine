use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::{
	num::NonZeroUsize,
	sync::{Arc, Mutex},
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext,
	structured::{Payload, StructuredBinding, StructuredContext},
	view::View,
};
#[derive(Deserialize, JsonSchema)]
struct Input {
	#[serde(default)]
	fail: bool,
}

struct Event;
struct TestView {
	context: StructuredContext,
	mode: &'static str,
}

impl View<String, String, Event> for TestView {
	type Completion = ();
	type Error = trine::error::Failure;
	async fn progress(&mut self, _: Event) -> Result<(), trine::error::Failure> {
		if self.mode == "early-progress" {
			let rejected = self
				.context
				.terminal(Payload::Success(json!("premature")))
				.await;
			assert!(rejected.is_err());
		}
		if self.mode == "progress-error" {
			return Err("writer failed".into());
		}
		Ok(())
	}

	async fn finish(&mut self, _: &Result<String, String>) -> Result<(), trine::error::Failure> {
		if self.mode == "missing" {
			return Ok(());
		}
		self.context
			.terminal(Payload::Success(json!("first")))
			.await?;
		if self.mode == "duplicate" {
			let _ = self
				.context
				.terminal(Payload::Success(json!("second")))
				.await;
		}
		Ok(())
	}
}

fn action() -> Action<Input, String, String, Event> {
	Action::new(
		"run",
		"Run",
		|input: Input, context: InvocationContext<Event>| async move {
			for _ in 0..3 {
				if context.emit(Event).await.is_err() {
					return Err("cancelled".into());
				}
			}
			if input.fail {
				Err("domain failure".into())
			} else {
				Ok("done".into())
			}
		},
	)
	.unwrap()
}

#[test]
fn terminal_guard_rejects_missing_duplicate_and_late_writes() {
	for mode in ["missing", "duplicate"] {
		let retained = Arc::new(Mutex::new(None));
		let retain = retained.clone();
		let mut app = Application::new("demo", "1").unwrap();
		app.register(
			Entry::new(action()).rpc(StructuredBinding::custom(move |context| {
				*retain.lock().unwrap() = Some(context.clone());
				Ok(TestView { context, mode })
			})),
		)
		.unwrap();
		let report = app
			.invoke_rpc(&["run"], json!({}), false, Cancellation::new())
			.unwrap();
		assert!(report.execution.domain_succeeded);
		assert!(!report.execution.presentation_succeeded);
		let typed = report
			.execution
			.typed::<String, String, trine::error::Failure>()
			.unwrap();
		assert!(
			format!("{:?}", typed.presentation).contains(if mode == "missing" {
				"no terminal"
			} else {
				"duplicate"
			})
		);
		assert_eq!(
			report.transcript.terminal,
			if mode == "missing" {
				None
			} else {
				Some(Payload::Success(json!("first")))
			}
		);
		let context = retained.lock().unwrap().take().unwrap();
		assert!(smol::block_on(context.terminal(Payload::Success(json!("late")))).is_err());
		assert!(smol::block_on(context.progress(json!("late"))).is_err());
	}
}

#[test]
fn default_domain_failure_is_not_presentation_failure() {
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action()).mcp(StructuredBinding::serde()))
		.unwrap();
	let report = app
		.invoke_mcp(&["run"], json!({"fail":true}), false, Cancellation::new())
		.unwrap();
	assert!(!report.execution.domain_succeeded);
	assert!(report.execution.presentation_succeeded);
	assert_eq!(
		report.transcript.terminal,
		Some(Payload::DomainFailure(json!("domain failure")))
	);
}

#[test]
fn progress_failure_wakes_emitters_and_preserves_settled_outcome() {
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action()).rpc(
			StructuredBinding::custom(|context| {
				Ok(TestView {
					context,
					mode: "progress-error",
				})
			})
			.observe(NonZeroUsize::new(1).unwrap()),
		),
	)
	.unwrap();
	let cancellation = Cancellation::new();
	let report = app
		.invoke_rpc(&["run"], json!({}), true, cancellation.clone())
		.unwrap();
	assert!(cancellation.is_cancelled());
	assert!(!report.execution.presentation_succeeded);
	assert_eq!(
		report
			.execution
			.typed::<String, String, trine::error::Failure>()
			.unwrap()
			.outcome,
		Err("cancelled".into())
	);
	assert!(report.transcript.terminal.is_none());
}

#[test]
fn distinct_action_types_and_invocation_transcripts_are_isolated() {
	#[derive(Deserialize, JsonSchema)]
	struct ListInput {
		label: String,
	}
	let list = Action::new(
		"list",
		"List",
		|input: ListInput, _: InvocationContext<()>| async { Ok::<_, bool>(vec![input.label]) },
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action()).rpc(StructuredBinding::serde()))
		.unwrap();
	app.register(Entry::new(list).rpc(StructuredBinding::serde()))
		.unwrap();
	for label in ["first", "second"] {
		let report = app
			.invoke_rpc(
				&["list"],
				json!({"label":label}),
				false,
				Cancellation::new(),
			)
			.unwrap();
		assert_eq!(
			report.transcript.terminal,
			Some(Payload::Success(json!([label])))
		);
		assert_eq!(
			report
				.execution
				.typed::<Vec<String>, bool, trine::error::Failure>()
				.unwrap()
				.outcome,
			Ok(vec![label.into()])
		);
	}
}

#[test]
fn action_unwind_closes_retained_invocation_sink() {
	let retained = Arc::new(Mutex::new(None));
	let retain = retained.clone();
	let action: Action<Input, String, String, Event> =
		Action::new("panic", "Panic", |_, _| async { panic!("action panic") }).unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action).rpc(StructuredBinding::custom(move |context| {
			*retain.lock().unwrap() = Some(context.clone());
			Ok(TestView {
				context,
				mode: "normal",
			})
		})),
	)
	.unwrap();
	let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
		app.invoke_rpc(&["panic"], json!({}), true, Cancellation::new())
	}));
	assert!(!result.unwrap().unwrap().execution.domain_succeeded);
	let context = retained.lock().unwrap().take().unwrap();
	assert!(smol::block_on(context.progress(json!("late"))).is_err());
	assert!(smol::block_on(context.terminal(Payload::Success(json!("late")))).is_err());
}

#[test]
fn early_terminal_attempts_remain_failures_when_the_view_ignores_them() {
	for mode in ["early-factory", "early-progress"] {
		let mut app = Application::new("demo", "1").unwrap();
		app.register(
			Entry::new(action()).rpc(
				StructuredBinding::custom(move |context| {
					if mode == "early-factory" {
						let rejected =
							smol::block_on(context.terminal(Payload::Success(json!("premature"))));
						assert!(rejected.is_err());
					}
					Ok(TestView { context, mode })
				})
				.observe(NonZeroUsize::new(1).unwrap()),
			),
		)
		.unwrap();
		let report = app
			.invoke_rpc(&["run"], json!({}), true, Cancellation::new())
			.unwrap();
		assert!(report.execution.domain_succeeded);
		assert!(!report.execution.presentation_succeeded);
		let typed = report
			.execution
			.typed::<String, String, trine::error::Failure>()
			.unwrap();
		assert!(format!("{:?}", typed.presentation).contains("only allowed during finish"));
		// A premature payload never occupies the terminal slot: finish can submit its own.
		assert_eq!(
			report.transcript.terminal,
			Some(Payload::Success(json!("first")))
		);
	}
}

#[test]
fn custom_diagnostic_preserves_an_accepted_terminal_and_original_failure() {
	use trine::{application::Interface, diagnostics::DiagnosticRenderer, error::FailureKind};
	let mut app = Application::new("demo", "1").unwrap();
	app.diagnostic_renderer(
		Interface::Rpc,
		DiagnosticRenderer::new(|diagnostic| {
			assert_eq!(
				diagnostic.presentation_failure,
				Some(FailureKind::ContractViolation)
			);
			Ok("custom recovery guidance".into())
		}),
	);
	app.register(
		Entry::new(action()).rpc(StructuredBinding::custom(|context| {
			Ok(TestView {
				context,
				mode: "duplicate",
			})
		})),
	)
	.unwrap();
	let report = app
		.invoke_rpc(&["run"], json!({}), false, Cancellation::new())
		.unwrap();
	assert_eq!(
		report.transcript.terminal,
		Some(Payload::Success(json!("first")))
	);
	assert_eq!(report.transcript.diagnostics, ["custom recovery guidance"]);
	assert!(
		report
			.execution
			.presentation_cause()
			.unwrap()
			.to_string()
			.contains("duplicate")
	);
	assert_eq!(report.execution.exit_code(), 1);
	assert!(report.execution.diagnostics.failure.is_none());
}

#[test]
fn both_execution_and_presentation_failures_survive_host_diagnostics() {
	let action: Action<Input, String, String, Event> = Action::new(
		"run",
		"Run",
		|_, context: InvocationContext<Event>| async move {
			context.emit(Event).await.unwrap();
			panic!("handler exploded");
		},
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action).rpc(
			StructuredBinding::custom(|context| {
				Ok(TestView {
					context,
					mode: "progress-error",
				})
			})
			.observe(NonZeroUsize::new(1).unwrap()),
		),
	)
	.unwrap();
	let report = app
		.invoke_rpc(&["run"], json!({}), true, Cancellation::new())
		.unwrap();
	assert!(report.execution.execution_failure.is_some());
	assert!(report.execution.presentation_failure().is_some());
	assert!(report.transcript.terminal.is_none());
	assert_eq!(report.transcript.diagnostics.len(), 1);
	assert!(report.transcript.diagnostics[0].contains("handler exploded"));
	assert!(report.transcript.diagnostics[0].contains("writer failed"));
}

#[test]
fn serde_progress_requires_invocation_opt_in_and_preserves_terminal() {
	#[derive(serde::Serialize)]
	struct Update {
		step: usize,
	}
	let action = Action::new(
		"updates",
		"Updates",
		|_: Input, context: InvocationContext<Update>| async move {
			context.emit(Update { step: 1 }).await.unwrap();
			Ok::<_, ()>("complete")
		},
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::serde_progress(
		NonZeroUsize::new(1).unwrap(),
	)))
	.unwrap();

	for observe in [false, true] {
		let report = app
			.invoke_rpc(&["updates"], json!({}), observe, Cancellation::new())
			.unwrap();
		assert!(report.execution.presentation_succeeded);
		assert_eq!(
			report.transcript.progress,
			if observe {
				vec![json!({"step":1})]
			} else {
				vec![]
			}
		);
		assert_eq!(
			report.transcript.terminal,
			Some(Payload::Success(json!("complete")))
		);
	}
}

#[test]
fn serde_progress_encoding_failure_keeps_action_result_and_classifies_failure() {
	struct Unencodable;
	impl serde::Serialize for Unencodable {
		fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
			Err(serde::ser::Error::custom("progress cannot be encoded"))
		}
	}
	let action = Action::new(
		"updates",
		"Updates",
		|_: Input, context: InvocationContext<Unencodable>| async move {
			let _ = context.emit(Unencodable).await;
			Ok::<_, ()>("settled")
		},
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).rpc(StructuredBinding::serde_progress(
		NonZeroUsize::new(1).unwrap(),
	)))
	.unwrap();
	let report = app
		.invoke_rpc(&["updates"], json!({}), true, Cancellation::new())
		.unwrap();
	assert_eq!(
		report.execution.presentation_failure(),
		Some(trine::error::FailureKind::EncodingFailed)
	);
	assert!(report.transcript.terminal.is_none());
	assert!(report.transcript.progress.is_empty());
	assert!(report.execution.domain_succeeded);
	assert!(
		report
			.execution
			.presentation_cause()
			.unwrap()
			.to_string()
			.contains("progress cannot be encoded")
	);
}
