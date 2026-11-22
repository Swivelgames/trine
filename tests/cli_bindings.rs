//! CLI schema projections for grouped fields, aliases and scalar options.

#[allow(dead_code)]
mod support;

use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use support::workyard::{self, CreateInput, RepositoryInput};
use trine::{
	Action, InvocationContext,
	cli::{Alias, Bindings, Cli, Parsed},
};

fn parse(arguments: &[&str]) -> CreateInput {
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	match cli.parse(arguments.iter().copied()).unwrap() {
		Parsed::Input(input) => input,
		Parsed::Display(text) => panic!("expected input, got {text}"),
	}
}

#[test]
fn grouping_and_common_options_preserve_order() {
	let input = parse(&[
		"demo",
		"create",
		"--remote",
		"common",
		"--repo",
		"api",
		"--repo-name",
		"backend",
		"--repo-remote",
		"upstream",
		"--from",
		"main",
		"--repo=web",
		"--repo-from=stable",
		"feat/payments",
	]);
	assert_eq!(input.remote.as_deref(), Some("common"));
	assert_eq!(input.from.as_deref(), Some("main"));
	assert_eq!(
		input.repositories,
		vec![
			RepositoryInput {
				path: "api".into(),
				name: Some("backend".into()),
				remote: Some("upstream".into()),
				from: None
			},
			RepositoryInput {
				path: "web".into(),
				name: None,
				remote: None,
				from: Some("stable".into())
			},
		]
	);
	assert!(!input.checkout);
	assert!(!input.pull);
}

#[test]
fn equals_and_boolean_flags_match_separate_arguments() {
	assert_eq!(
		parse(&[
			"demo",
			"create",
			"feature",
			"--repo=api",
			"--repo-name=backend",
			"--remote=origin",
			"--from=main",
			"--tasks-root=workspace",
			"--state-dir=state",
			"--checkout",
			"--pull"
		]),
		parse(&[
			"demo",
			"create",
			"feature",
			"--repo",
			"api",
			"--repo-name",
			"backend",
			"--remote",
			"origin",
			"--from",
			"main",
			"--tasks-root",
			"workspace",
			"--state-dir",
			"state",
			"--checkout",
			"--pull"
		]),
	);
}

#[test]
fn alias_is_the_same_typed_input() {
	assert_eq!(
		parse(&["demo", "checkout", "feature", "--repo=api"]),
		parse(&["demo", "create", "feature", "--repo=api", "--checkout"])
	);
}

#[test]
fn duplicate_fields_and_orphan_overrides_are_actionable_errors() {
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	for (arguments, expected) in [
		(
			vec![
				"demo",
				"create",
				"feature",
				"--repo-name=backend",
				"--repo=api",
			],
			"--repo-name requires a preceding",
		),
		(
			vec![
				"demo",
				"create",
				"feature",
				"--repo=api",
				"--repo-name=one",
				"--repo-name=two",
			],
			"--repo-name was supplied more than once",
		),
		(
			vec![
				"demo",
				"create",
				"feature",
				"--repo=api",
				"--remote=one",
				"--remote=two",
			],
			"--remote was supplied more than once",
		),
		(
			vec![
				"demo",
				"create",
				"feature",
				"--repo=api",
				"--pull",
				"--pull",
			],
			"--pull was supplied more than once",
		),
	] {
		let error = cli.parse(arguments).unwrap_err();
		assert!(error.0.contains(expected), "{error}");
	}
}

#[test]
fn missing_values_and_required_fields_are_errors() {
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	for (arguments, expected) in [
		(vec!["demo", "create", "feature", "--repo"], "--repo"),
		(vec!["demo", "create", "--repo=api"], "<TASK>"),
		(vec!["demo", "create", "feature"], "repositories"),
	] {
		assert!(cli.parse(arguments).unwrap_err().0.contains(expected));
	}
}

#[test]
fn help_and_version_do_not_invoke_a_handler() {
	let action: workyard::CreateAction = Action::new("create", "Never invoke", |_, _| async {
		panic!("handler must not run")
	})
	.unwrap();
	let cli = workyard::cli(action);
	for arguments in [
		vec!["demo", "--help"],
		vec!["demo", "create", "--help"],
		vec!["demo", "--version"],
	] {
		assert!(matches!(cli.parse(arguments), Ok(Parsed::Display(_))));
	}
}

#[derive(Deserialize, JsonSchema)]
struct Simple {
	name: String,
}

fn simple_action() -> Action<Simple, (), (), ()> {
	Action::new(
		"run",
		"Simple action",
		|input: Simple, _: InvocationContext<()>| async move {
			let _ = input.name;
			Ok(())
		},
	)
	.unwrap()
}

#[test]
fn invalid_registrations_fail_before_clap_can_panic() {
	assert!(
		Action::<Simple, (), (), ()>::new("bad name", "description", |_, _| async { Ok(()) })
			.is_err()
	);
	for bindings in [
		Bindings::new(),
		Bindings::new().option("missing", "missing"),
		Bindings::new()
			.option("name", "name")
			.option("name", "other"),
		Bindings::new().option("name", "help"),
	] {
		assert!(Cli::new("demo", "1", simple_action(), bindings, vec![]).is_err());
	}
	assert!(
		Cli::new(
			"demo",
			"1",
			simple_action(),
			Bindings::new().option("name", "name"),
			vec![Alias::new("run")]
		)
		.is_err()
	);
	assert!(
		Cli::new(
			"demo",
			"1",
			simple_action(),
			Bindings::new().option("name", "name"),
			vec![Alias::new("other"), Alias::new("other")]
		)
		.is_err()
	);
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct Unsupported {
	count: usize,
}

#[test]
fn unsupported_schema_names_the_field() {
	let action: Action<Unsupported, (), (), ()> =
		Action::new("run", "Unsupported", |_, _| async { Ok(()) }).unwrap();
	let error = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new().option("count", "count"),
		vec![],
	)
	.err()
	.unwrap();
	assert!(error.0.contains("count"));
	assert!(error.0.contains("custom CLI normalization"));
}

#[cfg(unix)]
#[test]
fn textual_cli_rejects_invalid_unicode_and_custom_normalizer_preserves_it() {
	use std::{ffi::OsString, os::unix::ffi::OsStringExt};
	let raw = OsString::from_vec(vec![b'a', 0xff]);
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	assert!(
		cli.parse([
			OsString::from("demo"),
			"create".into(),
			"feature".into(),
			"--repo".into(),
			raw.clone()
		])
		.is_err()
	);
	let expected = raw.clone();
	let custom = Cli::custom(simple_action(), move |arguments| {
		assert_eq!(arguments[1], expected);
		Ok(Parsed::Input(Simple {
			name: "native-path-handled".into(),
		}))
	});
	assert!(matches!(
		custom.parse([OsString::from("demo"), raw]),
		Ok(Parsed::Input(_))
	));
}

#[test]
fn explicit_false_does_not_consume_a_positional_and_alias_conflicts_are_errors() {
	assert!(!parse(&["demo", "create", "--pull=false", "feature", "--repo=api"]).pull);
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	let error = cli
		.parse([
			"demo",
			"checkout",
			"feature",
			"--repo=api",
			"--checkout=false",
		])
		.unwrap_err();
	assert!(error.0.contains("conflicts with alias 'checkout'"));
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(transparent)]
struct Identifier(String);

#[derive(Debug, Deserialize, JsonSchema, PartialEq, Eq)]
struct Referenced {
	name: Option<Identifier>,
	enabled: Option<bool>,
	#[serde(default = "default_help")]
	help: String,
}

fn default_help() -> String {
	"shared-default".into()
}

#[test]
fn nullable_references_and_serde_defaults_are_preserved() {
	let action: Action<Referenced, (), (), ()> =
		Action::new("run", "Referenced scalars", |_, _| async { Ok(()) }).unwrap();
	let cli = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new()
			.option("name", "name")
			.option("enabled", "enabled")
			.option("help", "assistance"),
		vec![],
	)
	.unwrap();
	assert_eq!(
		cli.parse(["demo", "run"]).unwrap(),
		Parsed::Input(Referenced {
			name: None,
			enabled: None,
			help: default_help()
		})
	);
	assert_eq!(
		cli.parse(["demo", "run", "--name=api", "--enabled=false"])
			.unwrap(),
		Parsed::Input(Referenced {
			name: Some(Identifier("api".into())),
			enabled: Some(false),
			help: default_help()
		})
	);
}

#[test]
fn duplicate_alias_assignments_are_rejected() {
	assert!(
		Cli::new(
			"demo",
			"1",
			simple_action(),
			Bindings::new().option("name", "name"),
			vec![
				Alias::new("other")
					.set("name", "first")
					.set("name", "second")
			]
		)
		.is_err()
	);
}

#[cfg(windows)]
#[test]
fn textual_cli_rejects_unpaired_surrogates_and_custom_normalizer_preserves_them() {
	use std::{ffi::OsString, os::windows::ffi::OsStringExt};
	let raw = OsString::from_wide(&[0xd800]);
	let cli = workyard::cli(workyard::action(Duration::ZERO));
	assert!(
		cli.parse([
			OsString::from("demo"),
			"create".into(),
			"feature".into(),
			"--repo".into(),
			raw.clone()
		])
		.is_err()
	);
	let expected = raw.clone();
	let custom = Cli::custom(simple_action(), move |arguments| {
		assert_eq!(arguments[1], expected);
		Ok(Parsed::Input(Simple {
			name: "native-path-handled".into(),
		}))
	});
	assert!(matches!(
		custom.parse([OsString::from("demo"), raw]),
		Ok(Parsed::Input(_))
	));
}

#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct ReservedProperty {
	#[serde(rename = "literal[].name")]
	name: String,
}

#[test]
fn group_notation_in_a_literal_property_is_rejected_at_registration() {
	let action: Action<ReservedProperty, (), (), ()> =
		Action::new("run", "Literal property", |_, _| async { Ok(()) }).unwrap();
	let error = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new().option("literal[].name", "name"),
		vec![],
	)
	.err()
	.unwrap();
	assert!(error.0.contains("literal[].name"), "{error}");
	assert!(error.0.contains("reserved group notation"), "{error}");
}

#[derive(Deserialize)]
struct RecursiveNullable;

impl JsonSchema for RecursiveNullable {
	fn schema_name() -> std::borrow::Cow<'static, str> {
		"RecursiveNullable".into()
	}

	fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
		schemars::json_schema!({
			"type": "object",
			"properties": {
				"name": {
					"anyOf": [
						{ "type": "null" },
						{ "$ref": "#/properties/name" }
					]
				}
			}
		})
	}
}

#[test]
fn recursive_nullable_reference_returns_a_registration_error() {
	let action: Action<RecursiveNullable, (), (), ()> =
		Action::new("run", "Recursive schema", |_, _| async { Ok(()) }).unwrap();
	let error = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new().option("name", "name"),
		vec![],
	)
	.err()
	.unwrap();
	assert!(error.0.contains("name"), "{error}");
	assert!(error.0.contains("recursive"), "{error}");
}
