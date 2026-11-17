use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use trine::{
	Action, Application, Entry, InvocationContext, NativePath,
	application::CliBinding,
	cli::{
		Alias, Bindings,
		completion::{self, Context},
	},
	view::View,
};

#[derive(Deserialize, Serialize, JsonSchema)]
struct Repository {
	path: NativePath,
	name: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Input {
	task: String,
	from: Option<String>,
	repositories: Vec<Repository>,
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

fn application() -> Application {
	let action = Action::new(
		"amend",
		"Amend",
		|_: Input, _: InvocationContext<()>| async {
			panic!("no action");
		},
	)
	.unwrap();

	let binding = CliBinding::new(
		Bindings::new()
			.positional("task")
			.complete_group("task", "Tasks", |_, _| vec!["fix/one".into()])
			.option("from", "from")
			.group("repositories", "path", "source")
			.native_path("repositories[].path")
			.option("repositories[].name", "label"),
	)
	.alias(Alias::new("edit"));

	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action)
			.namespace(["task"])
			.cli(binding, |_| Ok(Human), |_| 0),
	)
	.unwrap();

	app
}

#[test]
fn children_require_an_anchor_value_before_the_cursor_not_matching_text() {
	let app = application();

	for (before, after, eligible) in [
		(vec![], vec![], false),
		(vec!["--source", "/repo"], vec![], true),
		(vec!["--source=/repo"], vec![], true),
		(vec!["--source", "some space"], vec![], true),
		(vec!["--source", "-"], vec![], true),
		(vec!["--source", "-option"], vec![], false),
		(vec!["--source="], vec![], false),
		(vec!["--source"], vec![], false),
		(vec![], vec!["--source", "/repo"], false),
		(vec!["--from=--source"], vec![], false),
		(vec!["--from", "--source", "/repo"], vec![], false),
		(vec!["--label", "--source", "/repo"], vec![], false),
		(vec!["--", "--source", "/repo"], vec![], false),
	] {
		for alias in ["amend", "edit"] {
			let mut arguments: Vec<OsString> =
				["demo", "task", alias, "fix/one"].map(Into::into).to_vec();
			arguments.extend(before.iter().map(OsString::from));
			let index = arguments.len();
			arguments.push("--la".into());
			arguments.extend(after.iter().map(OsString::from));
			let context = Context::new(arguments, index, std::env::current_dir().unwrap());

			let candidates = completion::complete(app.cli_command(), context).unwrap();
			assert_eq!(
				candidates.contains(&"--label".into()),
				eligible,
				"{alias}: {before:?} | {after:?}: {candidates:?}"
			);
		}
	}
}

#[test]
fn the_anchor_remains_available_and_parsing_is_unchanged() {
	let app = application();
	let context = Context::new(
		["demo", "task", "amend", ""].map(Into::into).to_vec(),
		3,
		std::env::current_dir().unwrap(),
	);

	let candidates = completion::complete(app.cli_command(), context).unwrap();
	assert!(candidates.contains(&OsString::from("fix/one")));
	assert!(candidates.contains(&"--source".into()));
	assert!(!candidates.contains(&"--label".into()));
	assert!(
		app.parse_cli([
			"demo",
			"task",
			"amend",
			"fix/one",
			"--source=/repo",
			"--label=api"
		])
		.is_ok()
	);
}

#[test]
fn detached_single_dash_is_also_accepted_by_ordinary_parsing() {
	let app = application();
	let parsed = app.parse_cli([
		"demo",
		"task",
		"amend",
		"fix/one",
		"--source",
		"-",
		"--label=api",
	]);

	assert!(parsed.is_ok());
}
