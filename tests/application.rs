use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
	io::{self, BufWriter, Write},
	num::NonZeroUsize,
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, NativePath,
	application::{CliBinding, CliViewContext, Interface, InvocationError, Output},
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
fn standard_cli_supports_json_without_rpc_or_serialized_progress() {
	#[derive(Serialize)]
	struct LocalOutput {
		ready: bool,
	}

	#[derive(Serialize)]
	struct Error;
	struct Progress;
	struct LocalView;

	impl View<LocalOutput, Error, Progress> for LocalView {
		type Completion = ();
		type Error = trine::error::Failure;
		async fn progress(&mut self, _: Progress) -> Result<(), trine::error::Failure> {
			Ok(())
		}
		async fn finish(
			&mut self,
			_: &Result<LocalOutput, Error>,
		) -> Result<(), trine::error::Failure> {
			Ok(())
		}
	}

	let action = Action::new(
		"local",
		"Local",
		|_: Input, _: InvocationContext<Progress>| async {
			Ok::<_, Error>(LocalOutput { ready: true })
		},
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).cli(
		binding(),
		|_| -> Result<LocalView, trine::error::Failure> {
			panic!("JSON must not construct the human view")
		},
		|result| u8::from(result.is_err()),
	))
	.unwrap();

	let Parsed::Input(prepared) = app
		.parse_cli(["demo", "local", "--path", "api", "--json"])
		.unwrap()
	else {
		panic!()
	};
	assert!(app.discover(Interface::Rpc).is_empty());

	let stdout = CaptureWriter::default();
	let stderr = CaptureWriter::default();
	let report = prepared
		.run(
			CliViewContext {
				stdout: Output::new(stdout.clone()),
				stderr: Output::new(stderr.clone()),
			},
			Cancellation::new(),
		)
		.unwrap();

	let document: serde_json::Value =
		serde_json::from_slice(&stdout.bytes.lock().unwrap()).unwrap();
	assert_eq!(
		document,
		json!({"exit_code": 0, "result": {"ready": true}, "error": null})
	);
	assert_eq!(report.exit_code(), 0);
	assert!(stderr.bytes.lock().unwrap().is_empty());
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

#[derive(Clone, Default)]
struct CaptureWriter {
	bytes: Arc<Mutex<Vec<u8>>>,
	fail_flush: bool,
}

impl Write for CaptureWriter {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.bytes.lock().unwrap().extend_from_slice(bytes);
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		if self.fail_flush {
			return Err(io::Error::other("flush failed"));
		}
		Ok(())
	}
}

struct OutputView(CliViewContext);

impl View<Input, String, Event> for OutputView {
	type Completion = ();
	type Error = io::Error;

	async fn progress(&mut self, _: Event) -> io::Result<()> {
		self.0.stderr.write_all(b"working\n")
	}

	async fn finish(&mut self, outcome: &Result<Input, String>) -> io::Result<()> {
		let bytes = serde_json::to_vec(outcome.as_ref().unwrap())?;
		self.0.stdout.write_all(&bytes)
	}
}

fn output_application() -> Application {
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action(Arc::default())).cli(
		binding(),
		|context| Ok(OutputView(context)),
		|result| u8::from(result.is_err()),
	))
	.unwrap();
	app
}

#[test]
fn buffered_cli_outputs_flush_and_stay_scoped_to_each_invocation() {
	let app = output_application();
	let captures: Vec<_> = (0..2)
		.map(|_| (CaptureWriter::default(), CaptureWriter::default()))
		.collect();
	for (index, path) in ["api", "web"].into_iter().enumerate() {
		let (stdout, stderr) = &captures[index];
		let context = CliViewContext {
			stdout: Output::new(BufWriter::new(stdout.clone())),
			stderr: Output::new(BufWriter::new(stderr.clone())),
		};
		// Keep the buffers alive past execution so dropping them cannot mask missing flushes.
		let retained = context.clone();
		let Parsed::Input(prepared) = app.parse_cli(["demo", "create", "--path", path]).unwrap()
		else {
			panic!()
		};
		let report = prepared.run(context, Cancellation::new()).unwrap();
		assert!(report.domain_succeeded && report.presentation_succeeded);
		assert_eq!(
			*stdout.bytes.lock().unwrap(),
			format!(r#"{{"path":"{path}","checkout":false}}"#).into_bytes()
		);
		assert_eq!(*stderr.bytes.lock().unwrap(), b"working\n");
		drop(retained);
	}
	// The second invocation must not append to either first-invocation output.
	assert_eq!(
		*captures[0].0.bytes.lock().unwrap(),
		br#"{"path":"api","checkout":false}"#
	);
	assert_eq!(*captures[0].1.bytes.lock().unwrap(), b"working\n");
}

#[test]
fn buffered_cli_flush_failure_preserves_successful_domain_outcome() {
	let app = output_application();
	let stdout = CaptureWriter {
		fail_flush: true,
		..Default::default()
	};
	let stderr = CaptureWriter::default();
	let context = CliViewContext {
		stdout: Output::new(BufWriter::new(stdout.clone())),
		stderr: Output::new(BufWriter::new(stderr.clone())),
	};
	let retained = context.clone();
	let Parsed::Input(prepared) = app.parse_cli(["demo", "create", "--path", "api"]).unwrap()
	else {
		panic!()
	};
	let report = prepared.run(context, Cancellation::new()).unwrap();
	assert!(report.domain_succeeded);
	assert!(!report.presentation_succeeded);
	assert!(report.diagnostics.attempted);
	assert!(report.diagnostics.failure.is_none());
	assert!(!report.diagnostics.fallback_attempted);
	let diagnostic = String::from_utf8(stderr.bytes.lock().unwrap().clone()).unwrap();
	assert!(diagnostic.starts_with("working\n"));
	assert!(diagnostic.contains("The action returned an outcome, but presentation failed."));
	assert!(diagnostic.contains("flush failed"));
	let typed = report
		.typed_completion::<Input, String, trine::error::Failure, trine::application::CliCompletion>(
		)
		.unwrap();
	assert_eq!(
		match &typed.outcome {
			trine::execution::ExecutionOutcome::Completed(Ok(value)) => value.path.clone(),
			_ => panic!("expected successful outcome"),
		},
		NativePath::new("api").unwrap()
	);
	assert!(format!("{:?}", typed.presentation).contains("flush failed"));
	assert!(!stdout.bytes.lock().unwrap().is_empty());
	drop(retained);
}

#[test]
fn cli_completion_overrides_domain_default_without_host_downcasts() {
	struct CompletionView;
	impl View<(), (), ()> for CompletionView {
		type Completion = ();
		type Error = io::Error;
		async fn progress(&mut self, _: ()) -> io::Result<()> {
			Ok(())
		}
		async fn finish(&mut self, _: &Result<(), ()>) -> io::Result<()> {
			Ok(())
		}
	}
	let action = |name| {
		Action::new(
			name,
			"Completion policy",
			|input: Input, _: InvocationContext<()>| async move {
				if input.checkout { Err(()) } else { Ok(()) }
			},
		)
		.unwrap()
	};
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action("default")).cli(
		binding(),
		|_| Ok(CompletionView),
		|result| u8::from(result.is_err()),
	))
	.unwrap();
	// Avoid duplicate checkout aliases: this second entry only needs the input fields.
	let explicit_binding = CliBinding::new(
		Bindings::new()
			.option("path", "path")
			.native_path("path")
			.option("checkout", "checkout"),
	);
	app.register(Entry::new(action("explicit")).cli(
		explicit_binding,
		|_| Ok(CompletionView),
		|_| 42,
	))
	.unwrap();
	for (name, fail, expected) in [
		("default", false, 0),
		("default", true, 1),
		("explicit", false, 42),
		("explicit", true, 42),
	] {
		let mut args = vec!["demo", name, "--path", "api"];
		if fail {
			args.push("--checkout");
		}
		let Parsed::Input(prepared) = app.parse_cli(args).unwrap() else {
			panic!()
		};
		let report = prepared
			.run(CliViewContext::default(), Cancellation::new())
			.unwrap();
		assert_eq!(report.exit_code(), expected);
		assert!(!report.diagnostics.attempted);
	}
}

#[test]
fn cli_custom_diagnostic_writer_failure_falls_back_without_losing_outcome() {
	use trine::{diagnostics::DiagnosticRenderer, error::FailureKind};
	struct RejectCustomDiagnostic(CaptureWriter);
	impl Write for RejectCustomDiagnostic {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			if bytes == b"custom CLI diagnostic\n" {
				return Err(io::Error::new(
					io::ErrorKind::BrokenPipe,
					"diagnostic rejected",
				));
			}
			self.0.write(bytes)
		}
		fn flush(&mut self) -> io::Result<()> {
			self.0.flush()
		}
	}
	let mut app = output_application();
	let renderer_calls = Arc::new(AtomicUsize::new(0));
	let calls = renderer_calls.clone();
	app.diagnostic_renderer(
		Interface::Cli,
		DiagnosticRenderer::new(move |diagnostic| {
			calls.fetch_add(1, Ordering::SeqCst);
			assert!(diagnostic.domain_returned_ok);
			assert_eq!(
				diagnostic.presentation_failure,
				Some(FailureKind::OutputFailed)
			);
			Ok("custom CLI diagnostic".into())
		}),
	);
	let stderr = CaptureWriter::default();
	let Parsed::Input(prepared) = app.parse_cli(["demo", "create", "--path", "api"]).unwrap()
	else {
		panic!("expected invocation")
	};
	let report = prepared
		.run(
			CliViewContext {
				stdout: Output::new(CaptureWriter {
					fail_flush: true,
					..Default::default()
				}),
				stderr: Output::new(RejectCustomDiagnostic(stderr.clone())),
			},
			Cancellation::new(),
		)
		.unwrap();
	assert!(report.domain_succeeded);
	assert_eq!(
		report.presentation_failure(),
		Some(FailureKind::OutputFailed)
	);
	assert!(
		report
			.presentation_cause()
			.unwrap()
			.to_string()
			.contains("flush failed")
	);
	assert_eq!(renderer_calls.load(Ordering::SeqCst), 1);
	assert!(report.diagnostics.attempted);
	assert_eq!(
		report.diagnostics.failure.as_ref().unwrap().kind,
		FailureKind::OutputFailed
	);
	assert!(report.diagnostics.fallback_attempted);
	assert!(report.diagnostics.fallback_failure.is_none());
	let text = String::from_utf8(stderr.bytes.lock().unwrap().clone()).unwrap();
	assert!(text.starts_with("working\n"));
	assert_eq!(text.matches("Diagnostic delivery failed").count(), 1);
	assert!(text.contains("diagnostic rejected"));
	assert!(text.contains("flush failed"));
	assert_eq!(report.exit_code(), 1);
}

#[test]
fn diagnostic_renderers_are_selected_by_invocation_interface() {
	use trine::{diagnostics::DiagnosticRenderer, error::Failure};

	struct FailingView;
	impl View<Input, String, Event> for FailingView {
		type Completion = ();
		type Error = Failure;

		async fn progress(&mut self, _: Event) -> Result<(), Failure> {
			Ok(())
		}
		async fn finish(&mut self, _: &Result<Input, String>) -> Result<(), Failure> {
			Err("presentation failed".into())
		}
	}

	let mut app = Application::new("demo", "1").unwrap();
	for (interface, message) in [
		(Interface::Cli, "CLI diagnostic"),
		(Interface::Rpc, "RPC diagnostic"),
		(Interface::Mcp, "MCP diagnostic"),
	] {
		app.diagnostic_renderer(
			interface,
			DiagnosticRenderer::new(move |_| Ok(message.into())),
		);
	}
	app.register(
		Entry::new(action(Arc::default()))
			.cli(
				binding(),
				|_| Ok(FailingView),
				|result| u8::from(result.is_err()),
			)
			.rpc(StructuredBinding::custom(|_| Ok(FailingView)))
			.mcp(StructuredBinding::custom(|_| Ok(FailingView))),
	)
	.unwrap();

	let stderr = CaptureWriter::default();
	let Parsed::Input(prepared) = app.parse_cli(["demo", "create", "--path", "api"]).unwrap()
	else {
		panic!("expected parsed invocation");
	};
	let cli = prepared
		.run(
			CliViewContext {
				stdout: Output::new(io::sink()),
				stderr: Output::new(stderr.clone()),
			},
			Cancellation::new(),
		)
		.unwrap();
	assert!(cli.domain_succeeded);
	assert_eq!(*stderr.bytes.lock().unwrap(), b"CLI diagnostic\n");

	let rpc = app
		.invoke_rpc(
			&["create"],
			json!({"path": "api"}),
			false,
			Cancellation::new(),
		)
		.unwrap();
	let mcp = app
		.invoke_mcp(
			&["create"],
			json!({"path": "api"}),
			false,
			Cancellation::new(),
		)
		.unwrap();
	assert!(rpc.execution.domain_succeeded && mcp.execution.domain_succeeded);
	assert_eq!(rpc.transcript.diagnostics, ["RPC diagnostic"]);
	assert_eq!(mcp.transcript.diagnostics, ["MCP diagnostic"]);
}
