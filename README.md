# Trine

**Write your action once. Serve users through a CLI, agents through MCP, and local processes through JSON-RPC.**

Trine is a lightweight Rust library with all three interfaces built in. You write a typed async handler and choose how its output appears in the terminal. Trine parses command-line arguments, advertises MCP tools, and routes JSON-RPC requests over stdio for interprocess communication. Every caller reaches the same handler.

## Register your first action

Add the dependencies for this example to your application's `Cargo.toml`:

```toml
[dependencies]
trine = "0.0.1"
schemars = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

The following catalog exposes a greeting through all three interfaces. The CLI accepts a positional name and prints a line of text. RPC and MCP accept an object with a `name` field and use the built-in Serde presentation. The last few lines exercise the RPC binding in process, so you can run the example without starting a server.

```rust
use std::io;

use schemars::JsonSchema;
use serde::Deserialize;
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext,
	application::{CliBinding, CliViewContext},
	cli::Bindings,
	structured::{Payload, StructuredBinding},
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

	async fn progress(&mut self, _: ()) -> io::Result<()> {
		Ok(())
	}

	async fn finish(&mut self, result: &Result<String, String>) -> io::Result<()> {
		let (stream, text) = match result {
			Ok(greeting) => (&self.0.stdout, greeting),
			Err(message) => (&self.0.stderr, message),
		};
		stream.write_all(format!("{text}\n").as_bytes())
	}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let greet = Action::new(
		"greet",
		"Greet someone by name",
		|input: Greet, _: InvocationContext<()>| async move {
			Ok::<_, String>(format!("Hello, {}!", input.name))
		},
	)?;

	let cli = CliBinding::new(Bindings::new().positional("name"));
	let entry = Entry::new(greet)
		.cli(cli, |context| Ok(GreetingView(context)), |result| u8::from(result.is_err()))
		.rpc(StructuredBinding::serde())
		.mcp(StructuredBinding::serde());

	let mut app = Application::new("hello", "1.0")?;
	app.register(entry)?;

	let report = app.invoke_rpc(
		&["greet"],
		serde_json::json!({"name": "Ada"}),
		false,
		Cancellation::new(),
	)?;
	assert_eq!(report.transcript.terminal, Some(Payload::Success("Hello, Ada!".into())));
	Ok(())
}
```

## Connect an interface

Your executable chooses which interface to run. Enable each interface you want when building its entry.

| Caller | Entry point | What the caller sees |
| --- | --- | --- |
| Terminal user | Pass `std::env::args_os()` to `Application::parse_cli`. Print `Parsed::Display` for help or version. For `Parsed::Input`, call `run` with a `CliViewContext` and cancellation token, then use the report's `exit_code()`. | `hello greet Ada` prints `Hello, Ada!`. Adding `--json` selects Trine's machine-readable result and uses the same exit policy. |
| Local process | Run `trine::rpc::run_stdio(&app, shutdown)` in your IPC entry point. | Send one JSON-RPC object per line. The example below invokes the greeting. |
| MCP client | Configure the client to launch the entry point that calls `trine::mcp::run_stdio(&app, shutdown)`. | After initialization, `tools/list` advertises `greet` and its input schema. Call it through `tools/call`. |

```json
{"jsonrpc":"2.0","id":"1","method":"invoke","params":{"action":["greet"],"input":{"name":"Ada"}}}
```

Both stdio hosts use stdout for protocol traffic. Send application logs to stderr. To supply your own asynchronous reader and writer, use the corresponding module's `serve` function.

## When your action grows

- **Progress and cancellation.** Give the action an event type and emit progress through its `InvocationContext`. A CLI view can render each event. For structured output, choose `StructuredBinding::serde_with_progress` and a bounded capacity. A slow observer can make emission wait. Check the cancellation token at points where your operation can safely stop.
- **Commands and flags.** Map schema fields to positionals, named options or repeated groups with `Bindings`. Use `Entry::namespace` for nested commands. The command tree also supplies shell completions.
- **Filesystem paths.** Use `NativePath` when an argument must round-trip an OS path, including names outside Unicode. Mark the binding with `Bindings::native_path` for native CLI input.
- **Interactive decisions.** Put confirmation and selection requests in the action. Views decide how to answer them. Enable the optional `cli-confirmation` Cargo feature for terminal prompt helpers.
- **Interface-specific inputs.** Use `Entry::mcp_with_input` to advertise a smaller tool schema and translate it to the action's input. A custom structured view can also shape the terminal payload.

[API reference](https://docs.rs/trine/0.0.1/trine/) · MIT licensed
