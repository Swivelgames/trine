use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
	ffi::OsString,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Application, Entry, InvocationContext, NativePath,
	application::CliBinding,
	cli::{
		Bindings,
		completion::{self, Context, OptionValue},
	},
	view::View,
};

#[derive(Deserialize, Serialize, JsonSchema)]
struct Input {
	task: String,
	state: Option<NativePath>,
}

struct Human;

impl View<(), (), ()> for Human {
	type Completion = ();
	type Error = std::io::Error;

	async fn progress(&mut self, _: ()) -> Result<(), Self::Error> {
		panic!("no view")
	}

	async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), Self::Error> {
		panic!("no view")
	}
}

fn application(calls: Arc<AtomicUsize>) -> Application {
	let action = Action::new(
		"amend",
		"Amend",
		|_: Input, _: InvocationContext<()>| async {
			panic!("completion must not run actions");
		},
	)
	.unwrap();

	let bindings = Bindings::new()
		.positional("task")
		.option("state", "state-dir")
		.native_path("state")
		.complete("task", move |context, current| {
			calls.fetch_add(1, Ordering::SeqCst);
			assert_eq!(current, "fix/");
			assert!(
				matches!(context.option("state-dir"), OptionValue::Present(value) if value == "some state")
			);

			vec!["fix/one".into(), "fix/two".into()]
		});

	let mut application = Application::new("demo", "1").unwrap();
	application
		.register(Entry::new(action).namespace(["task"]).cli(
			CliBinding::new(bindings),
			|_| -> Result<Human, trine::error::Failure> { panic!("no factory") },
			|_| 0,
		))
		.unwrap();

	application
}

#[test]
fn providers_receive_trailing_native_context_without_action_admission() {
	let calls = Arc::new(AtomicUsize::new(0));
	let app = application(calls.clone());

	for arguments in [
		vec!["demo", "task", "amend", "fix/", "--state-dir", "some state"],
		vec!["demo", "task", "amend", "fix/", "--state-dir=some state"],
	] {
		let context = Context {
			arguments: arguments.into_iter().map(Into::into).collect(),
			index: 3,
			cwd: std::env::current_dir().unwrap(),
		};

		assert_eq!(
			completion::complete(app.cli_command(), context).unwrap(),
			vec![OsString::from("fix/one"), "fix/two".into()]
		);
	}

	assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn placement_options_preserve_literals_and_reject_ambiguity() {
	for (arguments, valid) in [
		(vec!["demo", "amend", "fix/", "--state-dir=x"], true),
		(vec!["demo", "amend", "fix/", "--state-dir", "x"], true),
		(vec!["demo", "amend", "fix/", "--state-dir"], false),
		(
			vec!["demo", "amend", "fix/", "--state-dir", "--json"],
			false,
		),
		(
			vec!["demo", "amend", "fix/", "--state-dir=x", "--state-dir=x"],
			false,
		),
	] {
		let context = Context {
			arguments: arguments.into_iter().map(Into::into).collect(),
			index: 2,
			cwd: std::env::current_dir().unwrap(),
		};

		assert_eq!(
			matches!(context.option("state-dir"), OptionValue::Present(value) if value == "x"),
			valid
		);
	}

	let context = Context {
		arguments: ["demo", "amend", "--", "--state-dir=x"]
			.map(Into::into)
			.to_vec(),
		index: 3,
		cwd: std::env::current_dir().unwrap(),
	};

	assert!(matches!(context.option("state-dir"), OptionValue::Absent));
}

#[cfg(unix)]
#[test]
fn equals_placement_keeps_non_unicode_path_bytes() {
	use std::{
		ffi::OsStr,
		os::unix::ffi::{OsStrExt, OsStringExt},
	};

	let context = Context {
		arguments: vec![
			"demo".into(),
			OsString::from_vec(b"--state-dir=a\xff".to_vec()),
			"fix/".into(),
		],
		index: 2,
		cwd: std::env::current_dir().unwrap(),
	};

	assert!(
		matches!(context.option("state-dir"), OptionValue::Present(value) if value == OsStr::from_bytes(b"a\xff"))
	);
}
