//! Catalog registration and dispatch across interfaces, aliases and native inputs.

#[path = "application/presentation.rs"]
mod presentation;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
	num::NonZeroUsize,
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, NativePath,
	application::{CliBinding, CliViewContext, Interface, InvocationError},
	cli::{Alias, Bindings, Parsed},
	structured::{Payload, StructuredBinding, StructuredContext},
	view::View,
};

#[derive(Debug, Deserialize, JsonSchema, Serialize, PartialEq)]
struct Input {
	path: NativePath,
	#[serde(default)]
	checkout: bool,
}

struct Event;
struct CliView {
	received: Arc<Mutex<Vec<Input>>>,
}

impl View<Input, String, Event> for CliView {
	type Completion = ();
	type Error = trine::error::Failure;
	async fn progress(&mut self, _: Event) -> Result<(), trine::error::Failure> {
		Ok(())
	}

	async fn finish(
		&mut self,
		result: &Result<Input, String>,
	) -> Result<(), trine::error::Failure> {
		let input = result.as_ref().unwrap();
		self.received.lock().unwrap().push(Input {
			path: input.path.clone(),
			checkout: input.checkout,
		});
		Ok(())
	}
}

fn binding() -> CliBinding {
	CliBinding::new(
		Bindings::new()
			.option("path", "path")
			.native_path("path")
			.option("checkout", "checkout"),
	)
	.alias(Alias::new("checkout").set("checkout", true))
}

fn action(calls: Arc<AtomicUsize>) -> Action<Input, Input, String, Event> {
	Action::new(
		"create",
		"Create",
		move |input: Input, context: InvocationContext<Event>| {
			calls.fetch_add(1, Ordering::SeqCst);
			async move {
				let _ = context.emit(Event).await;
				Ok(input)
			}
		},
	)
	.unwrap()
}

#[test]
fn shared_handler_selects_fresh_views_and_preserves_alias_and_native_input() {
	let calls = Arc::new(AtomicUsize::new(0));
	let factories = Arc::new(AtomicUsize::new(0));
	let received = Arc::new(Mutex::new(Vec::new()));
	let mut app = Application::new("demo", "1").unwrap();
	let output = received.clone();
	let count = factories.clone();
	app.register(
		Entry::new(action(calls.clone()))
			.namespace(["task"])
			.cli(
				binding(),
				move |_| {
					count.fetch_add(1, Ordering::SeqCst);
					Ok(CliView {
						received: output.clone(),
					})
				},
				|result| u8::from(result.is_err()),
			)
			.rpc(StructuredBinding::serde())
			.mcp(StructuredBinding::custom(|context| {
				Ok(CustomView { context })
			})),
	)
	.unwrap();
	for _ in 0..2 {
		let Parsed::Input(prepared) = app
			.parse_cli(["demo", "task", "checkout", "--path", "api"])
			.unwrap()
		else {
			panic!()
		};
		let report = prepared
			.run(CliViewContext::default(), Cancellation::new())
			.unwrap();
		assert!(report.domain_succeeded && report.presentation_succeeded);
	}
	assert_eq!(factories.load(Ordering::SeqCst), 2);
	let rpc = app
		.invoke_rpc(
			&["task", "create"],
			json!({"path":"api","checkout":true}),
			true,
			Cancellation::new(),
		)
		.unwrap();
	assert_eq!(
		rpc.transcript.terminal,
		Some(Payload::Success(json!({"path":"api","checkout":true})))
	);
	assert!(rpc.transcript.progress.is_empty());
	let mcp = app
		.invoke_mcp(
			&["task", "create"],
			json!({"path":"api"}),
			false,
			Cancellation::new(),
		)
		.unwrap();
	assert_eq!(
		mcp.transcript.terminal,
		Some(Payload::Success(json!("custom")))
	);
	assert_eq!(
		received.lock().unwrap().as_slice(),
		[
			Input {
				path: NativePath::new("api").unwrap(),
				checkout: true
			},
			Input {
				path: NativePath::new("api").unwrap(),
				checkout: true
			}
		]
	);
	assert_eq!(calls.load(Ordering::SeqCst), 4);
}

struct CustomView {
	context: StructuredContext,
}

impl View<Input, String, Event> for CustomView {
	type Completion = ();
	type Error = trine::error::Failure;
	async fn progress(&mut self, _: Event) -> Result<(), trine::error::Failure> {
		Ok(())
	}

	async fn finish(&mut self, _: &Result<Input, String>) -> Result<(), trine::error::Failure> {
		self.context
			.terminal(Payload::Success(json!("custom")))
			.await
	}
}

#[test]
fn unavailable_invalid_and_help_do_not_construct_views_or_execute() {
	let calls = Arc::new(AtomicUsize::new(0));
	let factories = Arc::new(AtomicUsize::new(0));
	let count = factories.clone();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action(calls.clone()))
			.cli(
				binding(),
				move |_| {
					count.fetch_add(1, Ordering::SeqCst);
					Ok(CliView {
						received: Arc::default(),
					})
				},
				|result| u8::from(result.is_err()),
			)
			.rpc(StructuredBinding::serde()),
	)
	.unwrap();
	assert_eq!(app.discover(Interface::Cli).len(), 1);
	assert!(app.discover(Interface::Mcp).is_empty());
	assert!(matches!(
		app.invoke_mcp(
			&["create"],
			json!({"path":"api"}),
			false,
			Cancellation::new()
		),
		Err(InvocationError::Unavailable)
	));
	assert!(matches!(
		app.invoke_rpc(&["unknown"], json!({}), false, Cancellation::new()),
		Err(InvocationError::Unavailable)
	));
	assert!(matches!(
		app.invoke_rpc(&["create"], json!({}), false, Cancellation::new()),
		Err(InvocationError::Input(_))
	));
	assert!(app.parse_cli(["demo", "create"]).is_err());
	for arg in ["--help", "--version"] {
		assert!(matches!(
			app.parse_cli(["demo", arg]).unwrap(),
			Parsed::Display(_)
		));
	}
	assert_eq!(calls.load(Ordering::SeqCst), 0);
	assert_eq!(factories.load(Ordering::SeqCst), 0);
}

#[test]
fn factory_errors_precede_handlers_for_each_interface() {
	let calls = Arc::new(AtomicUsize::new(0));
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action(calls.clone()))
			.cli(
				binding(),
				|_| Err::<CliView, _>("no output".into()),
				|result| u8::from(result.is_err()),
			)
			.rpc(StructuredBinding::custom(|_| {
				Err::<CustomView, _>("no output".into())
			})),
	)
	.unwrap();
	let Parsed::Input(prepared) = app.parse_cli(["demo", "create", "--path", "api"]).unwrap()
	else {
		panic!()
	};
	assert!(matches!(
		prepared.run(CliViewContext::default(), Cancellation::new()),
		Err(InvocationError::Setup(_))
	));
	assert!(matches!(
		app.invoke_rpc(
			&["create"],
			json!({"path":"api"}),
			false,
			Cancellation::new()
		),
		Err(InvocationError::Setup(_))
	));
	assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn registration_failure_publishes_no_structured_or_cli_entry() {
	let mut app = Application::new("demo", "1").unwrap();
	let first: Action<Input, Input, String, Event> =
		Action::new("checkout", "Existing", |input, _| async { Ok(input) }).unwrap();
	app.register(
		Entry::new(first).cli(
			CliBinding::new(
				Bindings::new()
					.option("path", "path")
					.native_path("path")
					.option("checkout", "checkout"),
			),
			|_| {
				Ok(CliView {
					received: Arc::default(),
				})
			},
			|result| u8::from(result.is_err()),
		),
	)
	.unwrap();
	assert!(
		app.register(
			Entry::new(action(Arc::default()))
				.cli(
					binding(),
					|_| Ok(CliView {
						received: Arc::default()
					}),
					|result| u8::from(result.is_err())
				)
				.rpc(StructuredBinding::serde())
		)
		.is_err()
	);
	assert!(app.discover(Interface::Rpc).is_empty());
	assert!(app.parse_cli(["demo", "create", "--path", "api"]).is_err());
	let nested = Entry::new(action(Arc::default()))
		.namespace(["checkout"])
		.rpc(StructuredBinding::serde());
	assert!(app.register(nested).is_err());
	// A failed registration must neither remove the old route nor reserve the new one.
	assert!(matches!(
		app.parse_cli(["demo", "checkout", "--path", "api"])
			.unwrap(),
		Parsed::Input(_)
	));
	app.register(Entry::new(action(Arc::default())).rpc(StructuredBinding::serde()))
		.unwrap();
	assert_eq!(app.discover(Interface::Rpc).len(), 1);
	assert!(
		app.prepare_rpc(
			&["create"],
			json!({"path": "api"}),
			false,
			Cancellation::new()
		)
		.is_ok()
	);
}

#[test]
fn mapped_progress_requires_both_registration_and_invocation_opt_in() {
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action(Arc::default())).rpc(StructuredBinding::serde_with_progress(
			|_: Event| Ok(json!("phase")),
			NonZeroUsize::new(1).unwrap(),
		)),
	)
	.unwrap();
	for observe in [false, true] {
		let report = app
			.invoke_rpc(
				&["create"],
				json!({"path":"api"}),
				observe,
				Cancellation::new(),
			)
			.unwrap();
		assert_eq!(report.transcript.progress.len(), usize::from(observe));
		assert!(report.execution.presentation_succeeded);
	}
}

#[test]
fn application_preserves_host_native_paths_and_custom_assembly() {
	use std::ffi::OsString;
	#[cfg(unix)]
	let native = {
		use std::os::unix::ffi::OsStringExt;
		OsString::from_vec(vec![b'a', 255])
	};
	#[cfg(windows)]
	let native = {
		use std::os::windows::ffi::OsStringExt;
		OsString::from_wide(&[65, 0xd800])
	};
	let expected = NativePath::new(native.clone()).unwrap();
	let received = Arc::new(Mutex::new(Vec::new()));
	let output = received.clone();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action(Arc::default())).cli_with_input(
		binding(),
		|mut fields| {
			let path = fields.take_path("path").unwrap();
			Ok(Input {
				path,
				checkout: true,
			})
		},
		move |_| {
			Ok(CliView {
				received: output.clone(),
			})
		},
		|result| u8::from(result.is_err()),
	))
	.unwrap();
	let Parsed::Input(prepared) = app
		.parse_cli([
			OsString::from("demo"),
			"create".into(),
			"--path".into(),
			native,
		])
		.unwrap()
	else {
		panic!()
	};
	assert!(
		prepared
			.run(CliViewContext::default(), Cancellation::new())
			.unwrap()
			.domain_succeeded
	);
	assert_eq!(received.lock().unwrap()[0].path, expected);
	assert!(received.lock().unwrap()[0].checkout);
}
