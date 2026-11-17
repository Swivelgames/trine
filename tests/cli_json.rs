use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
	io::{self, Write},
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, NativePath,
	application::{CliBinding, CliViewContext, Output},
	cli::{Alias, Bindings, Parsed},
	view::View,
};

#[derive(Deserialize, JsonSchema, Serialize)]
struct Input {
	path: NativePath,
	#[serde(default)]
	checkout: bool,
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(bytes);
		Ok(bytes.len())
	}
	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

impl Capture {
	fn json(&self) -> Value {
		serde_json::from_slice(&self.0.lock().unwrap()).unwrap()
	}
	fn text(&self) -> String {
		String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
	}
}

struct Human;

impl View<Input, Input, ()> for Human {
	type Completion = ();
	type Error = io::Error;
	async fn progress(&mut self, _: ()) -> io::Result<()> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<Input, Input>) -> io::Result<()> {
		Ok(())
	}
}

fn input_binding() -> CliBinding {
	CliBinding::new(
		Bindings::new()
			.positional("path")
			.native_path("path")
			.option("checkout", "checkout"),
	)
}

fn application(factories: Arc<AtomicUsize>) -> Application {
	let action = Action::new(
		"create",
		"Create",
		|input: Input, context: InvocationContext<()>| async move {
			context.emit(()).await.unwrap();
			let answer = context
				.confirm(trine::invocation::Confirmation {
					code: "decision".into(),
					message: "Proceed?".into(),
				})
				.await
				.unwrap();
			assert_eq!(answer, None);
			Ok(input)
		},
	)
	.unwrap();
	let binding = input_binding().alias(Alias::new("checkout").set("checkout", true));
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).namespace(["task"]).cli(
		binding,
		move |_| {
			factories.fetch_add(1, Ordering::SeqCst);
			Ok(Human)
		},
		|_| 7,
	))
	.unwrap();
	app
}

struct InvocationOutput {
	exit_code: u8,
	stdout: Capture,
	stderr: Capture,
}

fn run(
	app: &Application,
	arguments: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
) -> InvocationOutput {
	let Parsed::Input(prepared) = app.parse_cli(arguments).unwrap() else {
		panic!("expected invocation")
	};
	let stdout = Capture::default();
	let stderr = Capture::default();
	let report = prepared
		.run(
			CliViewContext {
				stdout: Output::with_terminal(stdout.clone(), true),
				stderr: Output::new(stderr.clone()),
			},
			Cancellation::new(),
		)
		.unwrap();
	InvocationOutput {
		exit_code: report.exit_code(),
		stdout,
		stderr,
	}
}

#[test]
fn json_preserves_aliases_namespaces_and_shared_exit_without_human_factory() {
	let factories = Arc::new(AtomicUsize::new(0));
	let app = application(factories.clone());
	for args in [
		vec!["demo", "--json", "task", "checkout", "api"],
		vec!["demo", "task", "--json", "checkout", "api"],
		vec!["demo", "task", "checkout", "api", "--json"],
	] {
		let output = run(&app, args);
		assert_eq!(output.exit_code, 7);
		assert_eq!(
			output.stdout.json(),
			json!({"exit_code": 7, "result": {"path":"api", "checkout":true}, "error":null})
		);
		assert!(output.stdout.text().contains("\n    \""));
		assert!(output.stdout.text().ends_with('\n'));
		assert!(!output.stdout.text().contains('\t'));
		assert!(output.stderr.text().is_empty());
	}
	assert_eq!(factories.load(Ordering::SeqCst), 0);
	assert_eq!(run(&app, ["demo", "task", "create", "api"]).exit_code, 7);
	assert_eq!(factories.load(Ordering::SeqCst), 1);
}

#[test]
fn literal_json_path_after_separator_does_not_select_json() {
	let factories = Arc::new(AtomicUsize::new(0));
	let app = application(factories.clone());
	let args = ["demo", "task", "create", "--", "--json"];
	assert!(!trine::cli::output_mode::json_requested(
		&args.map(Into::into)
	));
	let output = run(&app, args);
	assert!(output.stdout.text().is_empty());
	assert_eq!(factories.load(Ordering::SeqCst), 1);

	let output = run(&app, ["demo", "--json", "task", "create", "--", "--json"]);
	assert_eq!(output.stdout.json()["result"]["path"], "--json");
}

#[test]
fn json_preserves_native_path_encoding() {
	use std::ffi::OsString;
	#[cfg(unix)]
	let path = {
		use std::os::unix::ffi::OsStringExt;
		OsString::from_vec(vec![b'a', 255])
	};
	#[cfg(windows)]
	let path = {
		use std::os::windows::ffi::OsStringExt;
		OsString::from_wide(&[65, 0xd800])
	};
	let app = application(Arc::default());
	let expected = serde_json::to_value(NativePath::new(path.clone()).unwrap()).unwrap();
	let output = run(
		&app,
		[
			OsString::from("demo"),
			"--json".into(),
			"task".into(),
			"create".into(),
			path,
		],
	);
	assert_eq!(output.stdout.json()["result"]["path"], expected);
}

#[test]
fn execution_panic_has_framework_error_without_a_result() {
	let action: Action<Input, Input, Input, ()> =
		Action::new("panic", "Panic", |_, _| async { panic!("handler failed") }).unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).cli(input_binding(), |_| Ok(Human), |_| 0))
		.unwrap();
	let output = run(&app, ["demo", "panic", "api", "--json"]);
	assert_eq!(output.exit_code, 1);
	assert!(output.stdout.json()["result"].is_null());
	assert_eq!(output.stdout.json()["error"]["code"], "panicked");
}

#[test]
fn policy_panic_preserves_encoded_result_alongside_framework_error() {
	let action = Action::new(
		"create",
		"Create",
		|input: Input, _: InvocationContext<()>| async { Ok::<_, Input>(input) },
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).cli(
		input_binding(),
		|_| Ok(Human),
		|_| panic!("policy failed"),
	))
	.unwrap();
	let output = run(&app, ["demo", "create", "api", "--json"]);
	assert_eq!(output.exit_code, 1);
	assert_eq!(output.stdout.json()["result"]["path"], "api");
	assert_eq!(output.stdout.json()["error"]["code"], "panicked");
}

struct BrokenWriter;

impl Write for BrokenWriter {
	fn write(&mut self, _: &[u8]) -> io::Result<usize> {
		Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed stdout"))
	}
	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[test]
fn output_failure_retains_outcome_and_uses_stderr_fallback() {
	let app = application(Arc::default());
	let Parsed::Input(prepared) = app
		.parse_cli(["demo", "--json", "task", "create", "api"])
		.unwrap()
	else {
		panic!()
	};
	let stderr = Capture::default();
	let report = prepared
		.run(
			CliViewContext {
				stdout: Output::new(BrokenWriter),
				stderr: Output::new(stderr.clone()),
			},
			Cancellation::new(),
		)
		.unwrap();
	assert_eq!(report.exit_code(), 1);
	assert!(report.domain_succeeded);
	assert_eq!(
		report.presentation_failure(),
		Some(trine::error::FailureKind::OutputFailed)
	);
	assert!(stderr.text().contains("closed stdout"));
}

struct Unserializable;

impl Serialize for Unserializable {
	fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
		Err(serde::ser::Error::custom("cannot encode"))
	}
}
struct UnserializableView;

impl View<Unserializable, (), ()> for UnserializableView {
	type Completion = ();
	type Error = io::Error;
	async fn progress(&mut self, _: ()) -> io::Result<()> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<Unserializable, ()>) -> io::Result<()> {
		Ok(())
	}
}

#[test]
fn encoding_failure_emits_only_the_error_document() {
	let action = Action::new(
		"encode",
		"Encode",
		|_: Input, _: InvocationContext<()>| async { Ok::<_, ()>(Unserializable) },
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).cli(input_binding(), |_| Ok(UnserializableView), |_| 0))
		.unwrap();
	let output = run(&app, ["demo", "encode", "api", "--json"]);
	assert_eq!(output.exit_code, 1);
	assert!(output.stdout.json()["result"].is_null());
	assert_eq!(output.stdout.json()["error"]["code"], "encoding_failed");
	assert!(output.stderr.text().is_empty());
}

#[test]
fn json_option_is_reserved_for_standard_cli_presentation() {
	let action = Action::new(
		"conflicting",
		"Conflicting",
		|input: Input, _: InvocationContext<()>| async { Ok::<_, Input>(input) },
	)
	.unwrap();
	let binding = CliBinding::new(
		Bindings::new()
			.positional("path")
			.native_path("path")
			.option("checkout", "json"),
	);

	let mut app = Application::new("demo", "1").unwrap();
	let error = app
		.register(Entry::new(action).cli(binding, |_| Ok(Human), |_| 0))
		.unwrap_err();
	assert!(error.to_string().contains("--json is reserved"));
	assert!(app.discover(trine::application::Interface::Cli).is_empty());
}

#[test]
fn custom_native_assembly_and_cancelled_domain_result_survive_json() {
	let action = Action::new(
		"create",
		"Create",
		|input: Input, context: InvocationContext<()>| async move {
			context.cancellation().cancel();
			Ok::<_, Input>(input)
		},
	)
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	let binding = CliBinding::new(
		Bindings::new()
			.option("path", "path")
			.native_path("path")
			.option("checkout", "checkout"),
	);
	app.register(Entry::new(action).cli_with_input(
		binding,
		|mut fields| {
			Ok(Input {
				path: fields.take_path("path").unwrap(),
				checkout: true,
			})
		},
		|_| Ok(Human),
		|_| 1,
	))
	.unwrap();

	let args = ["demo", "create", "--path=--json"];
	assert!(!trine::cli::output_mode::json_requested(
		&args.map(Into::into)
	));
	let output = run(&app, ["demo", "create", "--json", "--path=--json"]);
	assert_eq!(output.exit_code, 1);
	assert_eq!(
		output.stdout.json()["result"],
		json!({"path":"--json", "checkout":true})
	);
	assert!(output.stdout.json()["error"].is_null());
}

#[test]
fn completion_projection_preserves_native_hints_and_namespace_flags() {
	let app = application(Arc::default());
	let command = app.cli_command();
	let action = command
		.find_subcommand("task")
		.unwrap()
		.find_subcommand("create")
		.unwrap();
	let path = action
		.get_arguments()
		.find(|arg| arg.get_id() == "input-path")
		.unwrap();
	assert_eq!(path.get_value_hint(), clap::ValueHint::AnyPath);
	let candidates = trine::cli::completion::complete(
		command.clone(),
		trine::cli::completion::Context {
			arguments: ["demo", "task", ""].map(Into::into).to_vec(),
			index: 2,
			cwd: std::env::current_dir().unwrap(),
		},
	)
	.unwrap();
	assert!(candidates.contains(&"checkout".into()));
}
