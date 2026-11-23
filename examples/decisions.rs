//! Ask before replacing the contents of an existing file.

#[path = "support/host.rs"]
mod host;

use std::{error::Error, io, process::ExitCode};

use futures_lite::io::AsyncWriteExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, NativePath,
	application::{CliBinding, CliViewContext},
	cli::Bindings,
	invocation::Confirmation,
	structured::StructuredBinding,
	view::View,
};

#[derive(Deserialize, JsonSchema)]
struct Replace {
	path: NativePath,
	#[serde(default)]
	yes: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
	Replaced,
	Kept,
}

async fn replace(input: Replace, context: InvocationContext<()>) -> Result<Outcome, String> {
	if !input.yes {
		let question = Confirmation {
			code: "replace_file".into(),
			message: format!(
				"Replace the contents of {}?",
				input.path.as_path().display()
			),
		};
		match context.confirm(question).await.map_err(|e| e.to_string())? {
			Some(true) => {}
			Some(false) => return Ok(Outcome::Kept),
			None => return Err("Replacement requires explicit approval.".into()),
		}
	}

	if context.cancellation().is_cancelled() {
		return Ok(Outcome::Kept);
	}
	let mut file = smol::fs::OpenOptions::new()
		.write(true)
		.truncate(true)
		.open(input.path)
		.await
		.map_err(|e| e.to_string())?;
	file.write_all(b"new contents\n")
		.await
		.map_err(|e| e.to_string())?;
	file.flush().await.map_err(|e| e.to_string())?;
	Ok(Outcome::Replaced)
}

struct Terminal(CliViewContext);

impl View<Outcome, String, ()> for Terminal {
	type Completion = ();
	type Error = io::Error;

	async fn confirm(
		&mut self,
		request: Confirmation,
		token: Cancellation,
	) -> io::Result<Option<bool>> {
		self.0.confirm(&request, &token)
	}

	async fn finish(&mut self, result: &Result<Outcome, String>) -> io::Result<()> {
		match result {
			Ok(Outcome::Replaced) => self.0.stdout.write_all(b"Replaced.\n"),
			Ok(Outcome::Kept) => self.0.stdout.write_all(b"Kept original.\n"),
			Err(message) => self.0.stderr.write_all(format!("{message}\n").as_bytes()),
		}
	}
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
	let action = Action::new("replace", "Replace an existing file's contents", replace)?;
	let bindings = Bindings::new()
		.positional("path")
		.native_path("path")
		.option("yes", "yes");
	let entry = Entry::new(action)
		.cli(
			CliBinding::new(bindings),
			|context| Ok(Terminal(context)),
			|result| u8::from(result.is_err()),
		)
		.rpc(StructuredBinding::serde())
		.mcp(StructuredBinding::serde());

	let mut app = Application::new("decisions", "1")?;
	app.register(entry)?;
	host::run(app)
}
