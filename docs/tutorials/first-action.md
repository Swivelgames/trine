# Serve one action over three interfaces

Build a greeting once and call it as a person at a terminal, a script reading JSON, and an agent discovering an MCP tool. Run the commands from the root of a Trine checkout. The greeting’s handler, bindings and human view are in [examples/hello.rs][hello].

## Follow a name through the application

Start with the human command.

```sh
cargo run --quiet --example hello -- greet Ada
```

The terminal prints `Hello, Ada!`. In `application()`, follow the input from registration to output.

1. `Greet` declares a required string named `name`. Its derived schema tells a tool client what to send. Its deserializer gives the handler a Rust value.
2. `Bindings::new().positional("name")` maps the first argument after `greet` to that field. The handler trims the name and returns a greeting or a domain error for a blank name.
3. `Entry::cli` connects a factory for `GreetingView` and an exit policy. The view writes the greeting to stdout or the reason for failure to stderr. The policy gives failures exit code 1. Both structured bindings use Serde to present the handler's result.

Run the same action with the built-in JSON view.

```sh
cargo run --quiet --example hello -- greet Ada --json
```

```json
{"exit_code":0,"result":"Hello, Ada!","error":null}
```

Try a name consisting of a single space. The process exits with code 1. The message appears in `result` because the handler ran and rejected its input. The envelope's `error` field is for failures in Trine itself.

```sh
cargo run --quiet --example hello -- greet ' ' --json
```

## Call the action from another process

Run the on-demand RPC host in your terminal.

```sh
cargo run --quiet --example hello -- rpc
```

Enter this request as one line and press Enter.

```json
{"jsonrpc":"2.0","id":1,"method":"invoke","params":{"action":["greet"],"input":{"name":"Ada"}}}
```

The response has the same ID and a `result` of `"Hello, Ada!"`. A client can send another line with a fresh ID on the same connection. Close stdin to end the session (Ctrl-D on an empty line on Unix, or Ctrl-Z then Enter in a Windows console).

## Discover and call the MCP tool

Start a fresh process in MCP mode.

```sh
cargo run --quiet --example hello -- mcp
```

Act as a client by entering each frame on its own line. First negotiate the protocol version. Wait for the server's initialization response.

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"tutorial","version":"1"}}}
```

Acknowledge initialization and ask which tools are available. Only the request with an ID receives a response.

```json
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list"}
```

Find `greet` in `result.tools`. Its `inputSchema` advertises the same required `name` field you supplied on the command line. Use that schema to call it.

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"greet","arguments":{"name":"Ada"}}}
```

Read `result.content[0].text` as JSON to get the string `Hello, Ada!`. When an action returns a JSON object, Trine also includes that object in `result.structuredContent`. Close stdin after receiving the response. In an MCP client, configure the launch command as `cargo`, arguments as `["run","--quiet","--example","hello","--","mcp"]`, and working directory as your Trine checkout. Both protocol modes reserve stdout for frames.

## Change the presentation

Edit `GreetingView::finish` to add a prefix to the greeting. Run the human and JSON commands again. Only the human output picks up the prefix. Now change the greeting in the handler and repeat the RPC call. The new wording reaches every interface because they all invoke the same action.

[hello]: ../../examples/hello.rs
