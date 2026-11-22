//! Discovery and invocation for tools with a projected input schema.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext,
	application::{Interface, InvocationError},
	structured::{Payload, StructuredBinding},
};

#[derive(Deserialize, JsonSchema, Serialize)]
struct Input {
	task: String,
	force: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NamedTask {
	task: String,
}

#[test]
fn tool_discovery_and_dispatch_share_the_projected_input() {
	let action = Action::new(
		"recover",
		"Recover a workspace or task",
		|input: Input, _: InvocationContext<()>| async move { Ok::<_, String>(input) },
	)
	.unwrap();
	let entry = Entry::new(action)
		.namespace(["tasks"])
		.rpc(StructuredBinding::serde())
		.mcp_with_input(
			StructuredBinding::serde(),
			"Resume a named task",
			|input: NamedTask| Input {
				task: input.task,
				force: false,
			},
		)
		.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(entry).unwrap();

	let rpc = app.discover(Interface::Rpc)[0];
	let tool = app.discover(Interface::Mcp)[0];
	assert_eq!(tool.path, rpc.path);
	assert_eq!(rpc.description, "Recover a workspace or task");
	assert_eq!(tool.description, "Resume a named task");
	assert!(rpc.input_schema["properties"]["force"].is_object());
	assert!(tool.input_schema["properties"].get("force").is_none());
	assert_eq!(tool.input_schema["required"], json!(["task"]));

	let path = ["tasks", "recover"];
	let named = json!({"task":"feat/api"});
	let report = app
		.invoke_mcp(&path, named, false, Cancellation::new())
		.unwrap();
	assert_eq!(
		report.transcript.terminal,
		Some(Payload::Success(json!({"task":"feat/api","force":false})))
	);

	let forced = json!({"task":"feat/api","force":true});
	assert!(matches!(
		app.prepare_mcp(&path, forced.clone(), false, Cancellation::new()),
		Err(InvocationError::Input(_))
	));
	let rpc = app
		.invoke_rpc(&path, forced.clone(), false, Cancellation::new())
		.unwrap();
	assert_eq!(rpc.transcript.terminal, Some(Payload::Success(forced)));
}
