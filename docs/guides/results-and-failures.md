# Read an action result across interfaces

A nonzero exit code can mean the action rejected the input. It can also mean the action succeeded and its view could not write the answer. The distinction matters for a command that creates a file or changes a repository. Retrying it solely because stdout failed could repeat an effect that already happened.

## Follow one rejection

The [hello example][hello] accepts a string as its `name` input. A single space passes input decoding, reaches the handler, and produces `Err("Please supply a name.")` after trimming. Omitting the argument altogether would exercise the parser instead.

Run both CLI presentations from the Trine checkout.

```sh
cargo run --quiet --example hello -- greet ' '
cargo run --quiet --example hello -- greet ' ' --json
```

| Caller | What it receives | How to interpret it |
| --- | --- | --- |
| Human CLI | `Please supply a name.` on stderr, empty stdout, exit code 1 | The human view rendered the reason. The example's exit policy selected 1 for the domain error. |
| CLI with `--json` | `{"exit_code":1,"result":"Please supply a name.","error":null}` on stdout, exit code 1 | Read the application's outcome from `result`. Here the call was presented successfully, so the framework diagnostic in `error` is null. |
| JSON-RPC with the Serde binding | `error.code` is 1, `error.message` is `"Action failed"`, and `error.data` contains the reason | Read the action's serialized error from `data`. The top-level ID identifies the invocation. |
| MCP with the Serde binding | `result.isError` is true. Decoding `result.content[0].text` as JSON gives the reason. | The server successfully answered `tools/call` and the tool reported a failure. An object-valued action error also appears in `result.structuredContent`. |

To see the protocol responses, start the hosts as shown in the [first-action tutorial][first-action]. Send this line to the RPC host.

```json
{"jsonrpc":"2.0","id":2,"method":"invoke","params":{"action":["greet"],"input":{"name":" "}}}
```

After initializing the MCP connection, send the equivalent tool call.

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"greet","arguments":{"name":" "}}}
```

If callers need to branch on the reason, give the action a serializable error type with a stable code. The default structured bindings preserve its fields. The human view can still choose its own wording. In CLI JSON, consult `error` for framework failures even if `result` is present. For example, an exit policy can panic after the view captures the domain result.

## Fail the write after the action succeeds

Run this Python 3 snippet from the Trine checkout. It gives the process a pipe whose reader has already closed. Passing a valid name lets the handler finish successfully before `GreetingView::finish` attempts its first write.

```python
import os
import subprocess

reader, writer = os.pipe()
os.close(reader)
with os.fdopen(writer, "wb") as stdout:
	run = subprocess.run(
		["cargo", "run", "--quiet", "--example", "hello", "--", "greet", "Ada"],
		stdout=stdout, stderr=subprocess.PIPE, text=True,
	)

print(run.returncode)
print(run.stderr.strip())
```

The process exits with 1. The default diagnostic on stderr begins with `The action returned an outcome, but presentation failed.` and identifies `OutputFailed`. The operating system supplies the final I/O message. A caller that captures only stdout will miss this diagnostic. Even a JSON document buffered before writing can be lost or truncated if the destination fails during the write.

## Inspect the report inside a host

A host running a prepared CLI invocation can look beyond the exit code. In the broken-pipe case, `report.domain_succeeded` is true, `report.presentation_failure()` is `Some(FailureKind::OutputFailed)`, and `report.exit_code()` is 1. An ordinary domain rejection has a completed presentation, a false `domain_succeeded`, and no `execution_failure`. If the handler panics, `execution_failure` carries the diagnostic and there is no returned domain value for `finish` to render.

An in-process structured caller also has a report and can match the terminal payload directly. This example uses the default Serde binding for RPC.

```rust
use schemars::JsonSchema;
use serde::Deserialize;
use trine::{Action, Application, Cancellation, Entry, InvocationContext};
use trine::structured::{Payload, StructuredBinding};

fn main() -> Result<(), Box<dyn std::error::Error>> {
	#[derive(Deserialize, JsonSchema)]
	struct Greet {
		name: String,
	}

	let action = Action::new("greet", "Greet someone", |input: Greet, _: InvocationContext<()>| async move {
		let name = input.name.trim();
		if name.is_empty() {
			return Err("Please supply a name.");
		}
		Ok(format!("Hello, {name}!"))
	})?;
	let mut app = Application::new("hello", "1")?;
	app.register(Entry::new(action).rpc(StructuredBinding::serde()))?;

	let input = serde_json::json!({"name": " "});
	let call = app.invoke_rpc(&["greet"], input, false, Cancellation::new())?;
	assert!(!call.execution.domain_succeeded);
	assert!(call.execution.presentation_succeeded);
	assert_eq!(call.transcript.terminal, Some(Payload::DomainFailure("Please supply a name.".into())));
	Ok(())
}
```

Use the report when deciding what to tell the user or whether an operation can safely be retried. A host can inspect `presentation_cause()` for the underlying view error and `diagnostics` for the outcome of diagnostic delivery. A remote client that loses its connection may have neither fact. For a mutating action, check the resulting application state before retrying.

[hello]: ../../examples/hello.rs
[first-action]: ../tutorials/first-action.md
