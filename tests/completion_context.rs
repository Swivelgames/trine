use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
	ffi::OsString,
	sync::{Arc, Mutex},
};
use trine::{
	Action, Application, Entry, InvocationContext, NativePath,
	application::CliBinding,
	cli::{
		Alias, Bindings,
		completion::{self, Context, OptionValue},
	},
	view::View,
};

#[derive(Deserialize, Serialize, JsonSchema)]
struct Repo {
	path: NativePath,
	remote: Option<String>,
	from: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Input {
	task: String,
	#[serde(default)]
	projects: Vec<NativePath>,
	checkout: bool,
	state: Option<NativePath>,
	repos: Vec<Repo>,
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

fn observed(arguments: Vec<OsString>, index: usize, field: &str) -> Context {
	let capture = Arc::new(Mutex::new(None));
	let output = capture.clone();
	let action = Action::new(
		"create",
		"Create",
		|_: Input, _: InvocationContext<()>| async { panic!("no action") },
	)
	.unwrap();

	let bindings = Bindings::new()
		.positional("task")
		.positional("projects")
		.native_path("projects")
		.option("checkout", "checkout")
		.option("state", "state-dir")
		.native_path("state")
		.group("repos", "path", "repo")
		.native_path("repos[].path")
		.option("repos[].remote", "repo-remote")
		.option("repos[].from", "repo-from")
		.complete(field, move |context, _| {
			*output.lock().unwrap() = Some(context.clone());
			Vec::new()
		});

	let mut app = Application::new("demo", "1").unwrap();
	app.register(Entry::new(action).namespace(["task"]).cli(
		CliBinding::new(bindings).alias(Alias::new("checkout").set("checkout", true)),
		|_| Ok(Human),
		|_| 0,
	))
	.unwrap();

	completion::complete(
		app.cli_command(),
		Context::new(arguments, index, std::env::current_dir().unwrap()),
	)
	.unwrap();

	capture.lock().unwrap().take().expect("provider invoked")
}

fn text(value: OptionValue<'_>) -> Option<&str> {
	match value {
		OptionValue::Present(value) => value.to_str(),
		_ => None,
	}
}

#[test]
fn group_ownership_retains_earlier_row_and_trailing_scalar_options() {
	let args = [
		"demo",
		"task",
		"create",
		"name",
		"--repo=first",
		"--repo-remote",
		"o",
		"--repo-from=local",
		"--repo",
		"second",
		"--repo-remote=upstream",
		"--state-dir=external",
	];
	let context = observed(args.map(Into::into).to_vec(), 6, "repos[].remote");

	let rows = context.groups("repos").unwrap();
	assert_eq!(rows.len(), 2);
	assert_eq!(text(rows[1].field("path")), Some("second"));

	let current = context.current_group("repos").unwrap();
	assert_eq!(text(current.field("path")), Some("first"));
	assert!(matches!(current.field("remote"), OptionValue::Incomplete));
	assert_eq!(text(current.field("from")), Some("local"));
	assert_eq!(text(context.option("state-dir")), Some("external"));
}

#[test]
fn fields_distinguish_absent_incomplete_and_ambiguous() {
	for (tail, expected) in [
		(vec![], "absent"),
		(vec!["--state-dir"], "incomplete"),
		(vec!["--state-dir=a", "--state-dir=b"], "ambiguous"),
	] {
		let mut args = vec!["demo", "task", "create", ""];
		args.extend(tail);
		let context = observed(args.into_iter().map(Into::into).collect(), 3, "task");

		let state = match context.field("state") {
			OptionValue::Absent => "absent",
			OptionValue::Incomplete => "incomplete",
			OptionValue::Ambiguous => "ambiguous",
			OptionValue::Present(_) => "present",
		};
		assert_eq!(state, expected);
	}
}

#[test]
fn aliases_supply_values_and_conflicts_or_duplicates_are_ambiguous() {
	for (tail, expected) in [
		(vec![], Some("true")),
		(vec!["--checkout"], Some("true")),
		(vec!["--checkout=false"], None),
		(vec!["--checkout", "--checkout"], None),
	] {
		let mut args = vec!["demo", "task", "checkout", ""];
		args.extend(tail);
		let context = observed(args.into_iter().map(Into::into).collect(), 3, "task");

		assert_eq!(text(context.field("checkout")), expected);
	}
}

#[test]
fn option_like_values_and_literal_separator_do_not_create_groups() {
	let args = [
		"demo",
		"task",
		"create",
		"",
		"--state-dir=--repo",
		"--",
		"--repo=decoy",
	];
	let context = observed(args.map(Into::into).to_vec(), 3, "task");

	assert!(context.groups("repos").unwrap().is_empty());
	assert_eq!(text(context.field("state")), Some("--repo"));
}

#[test]
fn malformed_detached_value_makes_group_context_unavailable() {
	for placement in [
		vec!["--state-dir", "--repo"],
		vec!["--state-dir=--repo", "--repo"],
	] {
		let inline = placement[0].contains('=');
		let mut args = vec!["demo", "task", "create", "name", "--repo", "first"];
		args.extend(placement);
		args.extend(["second", "--repo-remote", ""]);
		let index = args.len() - 1;
		let context = observed(
			args.into_iter().map(Into::into).collect(),
			index,
			"repos[].remote",
		);

		if inline {
			assert_eq!(
				text(context.current_group("repos").unwrap().field("path")),
				Some("second")
			);
			assert_eq!(text(context.field("state")), Some("--repo"));
		} else {
			assert!(context.groups("repos").is_none());
			assert!(context.current_group("repos").is_none());
			assert!(matches!(context.field("state"), OptionValue::Ambiguous));
		}
	}
}

#[cfg(unix)]
#[test]
fn grouped_and_scalar_paths_preserve_native_bytes() {
	use std::os::unix::ffi::OsStringExt;

	let path = OsString::from_vec(b"repo-\xff".to_vec());
	let mut state = OsString::from("--state-dir=");
	state.push(&path);
	let args = vec![
		"demo".into(),
		"task".into(),
		"create".into(),
		"name".into(),
		"--repo".into(),
		path.clone(),
		"--repo-from".into(),
		"".into(),
		state,
	];
	let context = observed(args, 7, "repos[].from");

	assert!(matches!(context.field("state"), OptionValue::Present(value) if value == path));
	assert!(
		matches!(context.current_group("repos").unwrap().field("path"), OptionValue::Present(value) if value == path)
	);
}

#[test]
fn a_variadic_provider_sees_completed_items_around_the_cursor_on_a_namespaced_alias() {
	let args = [
		"demo",
		"task",
		"checkout",
		"feature",
		"api",
		"--repo=explicit",
		"partial",
		"--repo-from=main",
		"web",
	];
	let context = observed(args.map(Into::into).to_vec(), 6, "projects");
	assert_eq!(text(context.field("task")), Some("feature"));
	assert_eq!(
		context.values("projects").unwrap(),
		["api", "web"].map(OsString::from)
	);
	assert_eq!(
		text(context.current_group("repos").unwrap().field("path")),
		Some("explicit")
	);
	assert_eq!(text(context.field("checkout")), Some("true"));
}

#[test]
fn the_next_value_of_a_variadic_field_is_completed_after_several_items() {
	let args = [
		"demo",
		"task",
		"create",
		"feature",
		"api",
		"--repo=explicit",
		"infra",
		"partial",
	];
	let context = observed(args.map(Into::into).to_vec(), 7, "projects");
	assert_eq!(text(context.field("task")), Some("feature"));
	assert_eq!(
		context.values("projects").unwrap(),
		["api", "infra"].map(OsString::from)
	);
}
