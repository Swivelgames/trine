use schemars::JsonSchema;
use serde::Deserialize;
use trine::{
	Action,
	cli::{Alias, Bindings, Cli, Parsed},
};

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
struct Input {
	task: String,
	#[serde(default)]
	accept_pull: Vec<String>,
	#[serde(default)]
	repositories: Vec<Repository>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
struct Repository {
	path: String,
	name: Option<String>,
}

fn action() -> Action<Input, (), (), ()> {
	Action::new("recover", "Repeated options", |_, _| async { Ok(()) }).unwrap()
}

fn bindings() -> Bindings {
	Bindings::new()
		.positional("task")
		.option("accept_pull", "accept-pull")
		.group("repositories", "path", "repo")
		.option("repositories[].name", "repo-name")
}

#[test]
fn repeated_strings_preserve_order_alongside_group_occurrences() {
	let cli = Cli::new("demo", "1", action(), bindings(), vec![]).unwrap();
	let input = cli
		.parse([
			"demo",
			"recover",
			"feature",
			"--accept-pull=web",
			"--repo=api",
			"--accept-pull",
			"api",
			"--repo-name=backend",
			"--accept-pull=web",
		])
		.unwrap();

	assert_eq!(
		input,
		Parsed::Input(Input {
			task: "feature".into(),
			accept_pull: vec!["web".into(), "api".into(), "web".into()],
			repositories: vec![Repository {
				path: "api".into(),
				name: Some("backend".into())
			}],
		})
	);
}

#[test]
fn omitted_repeated_option_preserves_serde_default_and_missing_value_fails() {
	let cli = Cli::new("demo", "1", action(), bindings(), vec![]).unwrap();
	assert_eq!(
		cli.parse(["demo", "recover", "feature"]).unwrap(),
		Parsed::Input(Input {
			task: "feature".into(),
			accept_pull: vec![],
			repositories: vec![],
		})
	);
	assert!(
		cli.parse(["demo", "recover", "feature", "--accept-pull"])
			.is_err()
	);
}

#[test]
fn arrays_reject_scalar_alias_assignments_and_must_be_the_last_positional() {
	assert!(
		Cli::new(
			"demo",
			"1",
			action(),
			bindings(),
			vec![Alias::new("other").set("accept_pull", "api")]
		)
		.is_err()
	);

	let positional = Bindings::new()
		.positional("accept_pull")
		.positional("task")
		.group("repositories", "path", "repo")
		.option("repositories[].name", "repo-name");
	assert!(Cli::new("demo", "1", action(), positional, vec![]).is_err());
}
