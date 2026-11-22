//! Heterogeneous actions and nested command paths in a single CLI registry.

use schemars::JsonSchema;
use serde::Deserialize;
use std::{
	num::NonZeroUsize,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{
	Action, Cancellation, InvocationContext,
	cli::{Alias, Bindings, Cli, Parsed},
	registry::Registry,
	view::View,
};
#[derive(Deserialize, JsonSchema)]
struct Greeting {
	name: String,
}
#[derive(Deserialize, JsonSchema)]
struct Toggle {
	#[serde(default)]
	enabled: bool,
}
struct GreetingView;
impl View<String, (), String> for GreetingView {
	type Completion = ();
	type Error = std::io::Error;
	async fn progress(&mut self, _: String) -> Result<(), std::io::Error> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<String, ()>) -> Result<(), std::io::Error> {
		Ok(())
	}
}
struct ToggleView;
impl View<bool, String, ()> for ToggleView {
	type Completion = ();
	type Error = std::io::Error;
	async fn progress(&mut self, _: ()) -> Result<(), std::io::Error> {
		Ok(())
	}
	async fn finish(&mut self, _: &Result<bool, String>) -> Result<(), std::io::Error> {
		Ok(())
	}
}
fn greeting() -> Cli<Greeting, String, (), String> {
	let action = Action::new(
		"greet",
		"Greeting",
		|input: Greeting, _: InvocationContext<String>| async move { Ok::<_, ()>(input.name) },
	)
	.unwrap();
	Cli::new(
		"demo",
		"1",
		action,
		Bindings::new().positional("name"),
		vec![Alias::new("hello")],
	)
	.unwrap()
}
fn registry() -> Registry {
	let mut registry = Registry::new("demo", "1").unwrap();
	registry
		.register(
			["task", "greet"],
			greeting(),
			|| GreetingView,
			NonZeroUsize::new(2),
		)
		.unwrap();
	let action = Action::new(
		"toggle",
		"Toggle",
		|input: Toggle, _: InvocationContext<()>| async move { Ok::<_, String>(input.enabled) },
	)
	.unwrap();
	let cli = Cli::new(
		"demo",
		"1",
		action,
		Bindings::new().option("enabled", "enabled"),
		vec![Alias::new("enable").set("enabled", true)],
	)
	.unwrap();
	registry
		.register(["toggle"], cli, || ToggleView, None)
		.unwrap();
	registry
}
#[test]
fn different_types_nested_paths_and_aliases_dispatch() {
	let registry = registry();
	for command in ["greet", "hello"] {
		let Parsed::Input(prepared) = registry.parse(["demo", "task", command, "Ada"]).unwrap()
		else {
			panic!()
		};
		let report = prepared.run(Cancellation::new());
		assert_eq!(
			report
				.typed::<String, (), std::io::Error>()
				.unwrap()
				.outcome,
			Ok("Ada".into())
		);
		assert!(report.domain_succeeded && report.presentation_succeeded);
	}
	let Parsed::Input(prepared) = registry.parse(["demo", "toggle", "--enabled"]).unwrap() else {
		panic!()
	};
	assert_eq!(
		prepared
			.run(Cancellation::new())
			.typed::<bool, String, std::io::Error>()
			.unwrap()
			.outcome,
		Ok(true)
	);
}
#[test]
fn duplicate_aliases_and_prefix_conflicts_are_rejected_atomically() {
	let mut registry = registry();
	for path in [
		vec!["task"],
		vec!["task", "greet"],
		vec!["task", "hello"],
		vec!["task", "greet", "child"],
	] {
		assert!(
			registry
				.register(path, greeting(), || GreetingView, None)
				.is_err()
		);
	}
	assert!(registry.parse(["demo", "task", "greet", "Ada"]).is_ok());
}
#[test]
fn help_invalid_inputs_and_fresh_view_lifetimes() {
	let creations = Arc::new(AtomicUsize::new(0));
	let counter = creations.clone();
	let mut registry = Registry::new("demo", "1").unwrap();
	registry
		.register(
			["greet"],
			greeting(),
			move || {
				counter.fetch_add(1, Ordering::SeqCst);
				GreetingView
			},
			None,
		)
		.unwrap();
	for args in [
		vec!["demo", "--help"],
		vec!["demo", "--version"],
		vec!["demo", "greet", "--help"],
	] {
		assert!(matches!(registry.parse(args), Ok(Parsed::Display(_))));
	}
	assert!(registry.parse(["demo", "greet"]).is_err());
	assert_eq!(creations.load(Ordering::SeqCst), 0);
	for _ in 0..2 {
		let Parsed::Input(prepared) = registry.parse(["demo", "greet", "Ada"]).unwrap() else {
			panic!()
		};
		prepared.run(Cancellation::new());
	}
	assert_eq!(creations.load(Ordering::SeqCst), 2);
}

#[test]
fn later_alias_collision_does_not_publish_primary_route() {
	let mut registry = registry();
	// The new primary is available, but its sibling alias collides with task hello.
	assert!(
		registry
			.register(["task", "new"], greeting(), || GreetingView, None)
			.is_err()
	);
	assert!(registry.parse(["demo", "task", "new", "Ada"]).is_err());

	let Parsed::Input(prepared) = registry.parse(["demo", "task", "hello", "Ada"]).unwrap() else {
		panic!("existing alias should still prepare its invocation");
	};
	let report = prepared.run(Cancellation::new());
	assert_eq!(
		report
			.typed::<String, (), std::io::Error>()
			.unwrap()
			.outcome,
		Ok("Ada".into())
	);
	assert!(report.presentation_succeeded);
}

#[test]
fn registry_alias_assignments_reach_handler_and_reject_conflicts() {
	let registry = registry();
	let Parsed::Input(prepared) = registry.parse(["demo", "enable"]).unwrap() else {
		panic!("alias should prepare its invocation");
	};
	let report = prepared.run(Cancellation::new());
	assert_eq!(
		report
			.typed::<bool, String, std::io::Error>()
			.unwrap()
			.outcome,
		Ok(true)
	);
	assert!(report.presentation_succeeded);

	let error = registry
		.parse(["demo", "enable", "--enabled=false"])
		.err()
		.unwrap();
	assert!(error.0.contains("conflicts with alias 'enable'"), "{error}");
}
