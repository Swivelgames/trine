//! Register list with a human CLI view and an MCP-specific summary.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io;
use trine::{
	Action, Application, Entry, InvocationContext, NativePath, RegistrationError,
	application::{CliBinding, CliViewContext},
	cli::Bindings,
	error::Failure,
	structured::{Payload, StructuredBinding, StructuredContext},
	view::View,
};

pub(super) fn register(app: &mut Application) -> Result<(), RegistrationError> {
	let action = Action::new(
		"list",
		"Simulate listing tasks",
		|input: ListInput, _: InvocationContext<ListProgress>| async {
			Ok::<_, String>(Listed {
				state_dir: input.state_dir,
				tasks: vec!["feat/payments".into()],
			})
		},
	)?;

	let cli = CliBinding::new(
		Bindings::new()
			.option("state_dir", "state-dir")
			.native_path("state_dir"),
	)
	.capacity(None);
	let mcp = StructuredBinding::custom(|context| Ok(ListMcpView { context }));
	let entry = Entry::new(action)
		.cli(
			cli,
			|output| Ok(ListView { output }),
			|result| u8::from(result.is_err()),
		)
		.rpc(StructuredBinding::serde())
		.mcp(mcp);

	app.register(entry)
}

#[derive(Deserialize, JsonSchema)]
struct ListInput {
	state_dir: NativePath,
}

#[derive(Serialize)]
struct Listed {
	state_dir: NativePath,
	tasks: Vec<String>,
}

struct ListProgress;

struct ListView {
	output: CliViewContext,
}

impl View<Listed, String, ListProgress> for ListView {
	type Completion = ();
	type Error = io::Error;

	async fn progress(&mut self, _: ListProgress) -> io::Result<()> {
		Ok(())
	}

	async fn finish(&mut self, result: &Result<Listed, String>) -> io::Result<()> {
		let response = match result {
			Ok(value) => serde_json::json!({"status":"completed","outcome":value}),
			Err(error) => serde_json::json!({"status":"failed","error":error}),
		};

		let mut bytes = serde_json::to_vec(&response)?;
		bytes.push(b'\n');
		self.output.stdout.write_all(&bytes)
	}
}

struct ListMcpView {
	context: StructuredContext,
}

impl View<Listed, String, ListProgress> for ListMcpView {
	type Completion = ();
	type Error = Failure;
	async fn progress(&mut self, _: ListProgress) -> Result<(), Failure> {
		Ok(())
	}
	async fn finish(&mut self, result: &Result<Listed, String>) -> Result<(), Failure> {
		let payload = match result {
			Ok(list) => Payload::Success(serde_json::json!({
				"summary": format!("{} task(s)", list.tasks.len()),
				"tasks": list.tasks,
			})),
			Err(error) => Payload::DomainFailure(serde_json::json!({"message":error})),
		};
		self.context.terminal(payload).await
	}
}
