//! An RPC caller can follow a directory scan, cancel it or wait for the final count.

use std::{error::Error, io, num::NonZeroUsize};

use futures_lite::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trine::{
	Action, Application, Cancellation, EmitError, Entry, InvocationContext, NativePath, rpc,
	structured::StructuredBinding,
};

#[derive(Deserialize, JsonSchema)]
struct Scan {
	directory: NativePath,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Summary {
	Complete { files: usize },
	Cancelled { files: usize },
}

async fn scan(input: Scan, context: InvocationContext<NativePath>) -> io::Result<Summary> {
	let mut files = 0;
	let mut entries = smol::fs::read_dir(input.directory).await?;

	while !context.cancellation().is_cancelled() {
		let Some(entry) = entries.try_next().await? else {
			return Ok(Summary::Complete { files });
		};
		if !entry.file_type().await?.is_file() {
			continue;
		}

		files += 1;
		let name = NativePath::new(entry.file_name()).map_err(io::Error::other)?;
		match context.emit(name).await {
			Ok(()) => {}
			Err(EmitError::Cancelled) => break,
			Err(EmitError::Closed) => return Err(io::Error::other("Progress receiver closed")),
		}
	}

	Ok(Summary::Cancelled { files })
}

fn main() -> Result<(), Box<dyn Error>> {
	let action = Action::new(
		"scan",
		"Count regular files in one directory",
		|input, ctx| async { scan(input, ctx).await.map_err(|error| error.to_string()) },
	)?;
	let capacity = NonZeroUsize::new(8).unwrap();
	let entry = Entry::new(action).rpc(StructuredBinding::serde_progress(capacity));

	let mut app = Application::new("directory-scan", "1.0")?;
	app.register(entry)?;
	rpc::run_stdio(&app, Cancellation::new())?;
	Ok(())
}
