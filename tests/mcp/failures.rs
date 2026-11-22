//! MCP request correlation and host availability after invocation failures.

use super::fixture::*;
use serde_json::Value;
use std::{
	fmt,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Entry, InvocationContext,
	application::Interface,
	diagnostics::DiagnosticRenderer,
	error::{Failure, FailureKind},
	structured::{Payload, StructuredBinding, StructuredContext},
	view::View,
};

#[test]
fn view_failure_has_a_correlated_error_and_the_host_remains_available() {
	struct Broken;
	impl serde::Serialize for Broken {
		fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
			Err(serde::ser::Error::custom("cannot serialize result"))
		}
	}
	let mut app = catalog();
	let broken = Action::new(
		"broken",
		"Broken serializer",
		|_: Input, _: InvocationContext<()>| async { Ok::<_, ()>(Broken) },
	)
	.unwrap();
	app.register(Entry::new(broken).mcp(StructuredBinding::serde()))
		.unwrap();

	let mut requests = ready();
	requests.extend([call(5, "broken"), call(6, "echo")]);
	let replies = exchange(&app, &requests);
	assert_eq!(replies[1]["id"], 5);
	assert_eq!(replies[1]["error"]["code"], -32603);
	assert!(
		replies[1]["error"]["message"]
			.as_str()
			.unwrap()
			.contains("cannot serialize result")
	);
	assert_eq!(replies[2]["id"], 6);
	assert!(replies[2]["result"]["structuredContent"].is_object());
}

#[test]
fn action_panic_uses_the_mcp_diagnostic_renderer() {
	let mut app = catalog();
	app.diagnostic_renderer(
		Interface::Mcp,
		DiagnosticRenderer::new(|_| Ok("custom agent diagnostic".into())),
	);
	let action = Action::new("panic", "Panic in action", panic_handler).unwrap();
	app.register(Entry::new(action).mcp(StructuredBinding::serde()))
		.unwrap();

	let mut requests = ready();
	requests.extend([call(2, "panic"), call(3, "echo")]);
	let result = exchange(&app, &requests);
	assert_eq!(result[1]["error"]["message"], "custom agent diagnostic");
	assert_eq!(result[1]["id"], 2);
	assert!(result[2]["result"]["structuredContent"].is_object());
}

async fn panic_handler(_: Input, _: InvocationContext<()>) -> Result<(), ()> {
	panic!("action panicked");
}

#[derive(Debug)]
struct BadDisplay;
impl fmt::Display for BadDisplay {
	fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
		panic!("formatting the setup error panicked")
	}
}
impl std::error::Error for BadDisplay {}

enum UnavailableView {}
impl View<(), (), ()> for UnavailableView {
	type Completion = ();
	type Error = Failure;
	async fn progress(&mut self, _: ()) -> Result<(), Failure> {
		match *self {}
	}
	async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), Failure> {
		match *self {}
	}
}

#[test]
fn a_panicking_setup_error_display_leaves_the_host_available() {
	let mut app = catalog();
	let action = Action::new(
		"setup",
		"Fail during setup",
		|_: Input, _: InvocationContext<()>| async { Ok::<(), ()>(()) },
	)
	.unwrap();
	let binding = StructuredBinding::custom(|_| {
		Err::<UnavailableView, _>(Failure::new(FailureKind::ViewSetupFailed, BadDisplay))
	});
	app.register(Entry::new(action).mcp(binding)).unwrap();

	let mut requests = ready();
	requests.extend([call(2, "setup"), call(3, "echo")]);
	let result = exchange(&app, &requests);
	assert_eq!(result[1]["error"]["code"], -32603);
	assert_eq!(result[1]["id"], 2);
	assert!(result[2]["result"]["structuredContent"].is_object());
}

struct InvertedResult(StructuredContext);

impl View<(), (), ()> for InvertedResult {
	type Completion = ();
	type Error = Failure;

	async fn progress(&mut self, _: ()) -> Result<(), Failure> {
		Ok(())
	}

	async fn finish(&mut self, outcome: &Result<(), ()>) -> Result<(), Failure> {
		let payload = match outcome {
			Ok(()) => Payload::DomainFailure(Value::Null),
			Err(()) => Payload::Success(Value::Null),
		};
		self.0.terminal(payload).await
	}
}

#[test]
fn contradictory_custom_results_are_presentation_errors_and_never_rerun_the_action() {
	for outcome in [Ok(()), Err(())] {
		let mut app = catalog();
		let runs = Arc::new(AtomicUsize::new(0));
		let observed = runs.clone();
		let action = Action::new(
			"inverted",
			"Return the outcome",
			move |_: Input, _: InvocationContext<()>| {
				observed.fetch_add(1, Ordering::Relaxed);
				async move { outcome }
			},
		)
		.unwrap();
		let binding = StructuredBinding::custom(|context| Ok(InvertedResult(context)));
		app.register(Entry::new(action).mcp(binding)).unwrap();

		let mut requests = ready();
		requests.push(call(2, "inverted"));
		let replies = exchange(&app, &requests);
		assert_eq!(replies.len(), 2);
		assert_eq!(replies[1]["id"], 2);
		assert_eq!(replies[1]["error"]["code"], -32603);
		assert_eq!(
			replies[1]["error"]["message"],
			"MCP tool presentation contradicts the action outcome"
		);
		assert!(replies[1].get("result").is_none());
		assert_eq!(runs.load(Ordering::Relaxed), 1);
	}
}
