//! One greeting served over the terminal, JSON-RPC and MCP.

#[path = "support/host.rs"]
mod host;

use std::{error::Error, io, process::ExitCode};

use schemars::JsonSchema;
use serde::Deserialize;
use trine::{
	Action, Application, Entry, InvocationContext,
	application::{CliBinding, CliViewContext},
	cli::Bindings,
	structured::StructuredBinding,
	view::View,
};

#[derive(Deserialize, JsonSchema)]
struct Greet {
	name: String,
}

struct GreetingView(CliViewContext);

impl View<String, String, ()> for GreetingView {
	type Error = io::Error;
	type Completion = ();

	async fn finish(&mut self, result: &Result<String, String>) -> io::Result<()> {
		let (stream, text) = match result {
			Ok(greeting) => (&self.0.stdout, greeting),
			Err(message) => (&self.0.stderr, message),
		};
		stream.write_all(format!("{text}\n").as_bytes())
	}
}

fn application() -> Result<Application, Box<dyn Error>> {
	let greet = Action::new(
		"greet",
		"Greet someone by name",
		|input: Greet, _: InvocationContext<()>| async move {
			let name = input.name.trim();
			if name.is_empty() {
				return Err(String::from("Please supply a name."));
			}

			Ok(format!("Hello, {name}!"))
		},
	)?;

	let cli = CliBinding::new(Bindings::new().positional("name"));
	let entry = Entry::new(greet)
		.cli(
			cli,
			|context| Ok(GreetingView(context)),
			|result| u8::from(result.is_err()),
		)
		.rpc(StructuredBinding::serde())
		.mcp(StructuredBinding::serde());

	let mut app = Application::new("hello", "1.0")?;
	app.register(entry)?;
	Ok(app)
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
	host::run(application()?)
}
