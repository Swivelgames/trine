//! Shell candidate generation from native input without executing actions.

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
		let context = Context::new(
			arguments.into_iter().map(Into::into).collect(),
			3,
			std::env::current_dir().unwrap(),
		);

		assert_eq!(
			completion::complete(app.cli_command(), context).unwrap(),
			vec![OsString::from("fix/one"), "fix/two".into()]
		);
	}

	assert_eq!(calls.load(Ordering::SeqCst), 2);
}
