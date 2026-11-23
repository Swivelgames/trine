# Choose how to host an action

Suppose an editor wants a Trine action. A Rust extension in the same process can invoke the registered action directly. A local helper process can accept a JSON-RPC request over pipes. An agent whose client discovers tools can launch an MCP host. The action can stay the same in all three cases. The choice determines who owns execution, where the result appears, and what closing the connection means.

| Caller or host | Entry point | What you own |
| --- | --- | --- |
| Rust caller holding a typed action and input | `Action::invoke(input, context).await` | Supply the invocation context and handle the returned domain `Result`. |
| Rust caller using a registered RPC or MCP binding | `Application::invoke_rpc` or `invoke_mcp` | Supply the action path and JSON input. The synchronous call returns a report and a transcript. |
| Application already running an async executor | `prepare_rpc` or `prepare_mcp`, then `.run().await` | Prepare the invocation, schedule its execution and await the report. |
| Executable using process stdin and stdout | `rpc::run_stdio` or `mcp::run_stdio` | Select a protocol, supply the application and shutdown token, and keep stdout available for frames. |
| Host with its own async reader and writer | `rpc::serve` or `mcp::serve` | Supply cancellation-safe streams for one connection and drive the server future through shutdown. |

## Embed an action and exercise its wire contract

Use a prepared call when your host already has an executor. Preparation looks up the registered binding, decodes its input and constructs its view. The handler starts when the host drives `run()`. Retain a clone of the cancellation token if another part of the application may need to stop the invocation.

```rust
use schemars::JsonSchema;
use serde::Deserialize;
use futures_lite::io::Cursor;
use serde_json::{Value, json};
use trine::{Action, Application, Cancellation, Entry, InvocationContext, rpc};
use trine::structured::{Payload, StructuredBinding};

fn main() -> Result<(), Box<dyn std::error::Error>> {
	#[derive(Deserialize, JsonSchema)]
	struct Version {}

	let version = Action::new(
		"version",
		"Read the catalog version",
		|_: Version, _: InvocationContext<()>| async { Ok::<_, String>("1.0") },
	)?;
	let mut app = Application::new("catalog", "1.0")?;
	app.register(Entry::new(version).rpc(StructuredBinding::serde()))?;

	let call = app.prepare_rpc(&["version"], json!({}), false, Cancellation::new())?;
	let report = smol::block_on(call.run());
	assert!(report.execution.domain_succeeded);
	assert_eq!(report.transcript.terminal, Some(Payload::Success("1.0".into())));

	let request = json!({
		"jsonrpc": "2.0", "id": 7, "method": "invoke",
		"params": {"action": ["version"], "input": {}}
	});
	let input = Cursor::new(format!("{request}\n").into_bytes());
	let mut output = Vec::new();
	let session = smol::block_on(rpc::serve(
		&app, input, &mut output, rpc::Config::default(), Cancellation::new(),
		|_| {},
	))?;

	assert!(session.failure.is_none());
	assert_eq!(session.invocations, 1);
	assert_eq!(serde_json::from_slice::<Value>(&output)?,
		json!({"jsonrpc": "2.0", "id": 7, "result": "1.0"}));
	Ok(())
}
```

Here `block_on` drives a standalone example. Inside an async function, use `call.run().await`. A synchronous embedding can get the same report in one step with `invoke_rpc`. The path selects the registered binding in both cases. There are no request IDs or wire frames in the transcript. Passing `true` for observation with a progress-capable binding retains progress values in the transcript until you release the report. For a long scan, request the terminal result alone if you only need the count.

RPC accepts a `Config` for admission and buffering limits. Its callback receives each settled invocation's report. The returned `HostReport` summarizes the connection, so check its `failure` field as well as the outer I/O result. For MCP, `mcp::serve(&app, reader, writer, shutdown).await` returns an I/O result. Use a reader you can keep open when testing an active MCP tool.

## Launch a subprocess

The [hello example][hello] chooses a host from its first argument. From the checkout, run one of these commands per connection.

```sh
cargo run --quiet --example hello -- rpc
```

```sh
cargo run --quiet --example hello -- mcp
```

An RPC client can immediately send an `invoke` request with an ID, action path and input. An MCP client first completes initialization, then discovers and calls tools. Both hosts exchange one JSON message per line. Flush each request and keep reading stdout while work is active, including when you ignore progress. A full output pipe can stall the server. Direct logs to stderr and drain that pipe too if your launcher captures it. For complete request and response examples, see the [first-action tutorial][first-action].

## Close a connection deliberately

| Event | JSON-RPC | MCP |
| --- | --- | --- |
| Client closes stdin cleanly and continues reading stdout | The host stops accepting requests and drains admitted calls. A running scan can finish and send its count. | The host requests cancellation of outstanding tools, waits for them to settle, and drains output already accepted for delivery. A withdrawn tool's eventual result is suppressed. |
| Host requests shutdown via its token | The host stops admission and requests cancellation of active calls. | The host withdraws active tools and requests cancellation. A stalled output drain can be interrupted. |

For a one-shot RPC helper, write the complete newline-terminated request, close the child's stdin, read its output to EOF, then wait for exit. For a one-shot MCP tool whose result you need, **keep stdin open until the response to `tools/call` arrives**. Closing it immediately after sending the call races the tool's completion.

Both hosts use cooperative cancellation. A handler doing a long scan must check its invocation token at a safe stopping point. Closing stdin or requesting shutdown cannot force that handler to return. The [directory scan tutorial][directory-scan] shows how to return a useful partial result when a caller explicitly cancels an RPC request.

[hello]: ../../examples/hello.rs
[first-action]: ../tutorials/first-action.md
[directory-scan]: ../tutorials/directory-scan.md
