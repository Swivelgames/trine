# Ask before replacing a file

A person can answer a terminal prompt. A script or an MCP client can supply approval with its request. The [decisions example][decisions] gives all three callers one action. It asks before opening the file for writing, and replaces its contents with a single line only after approval.

## Try both answers

From the Trine checkout in a Bash terminal, create a disposable file and choose **No** when prompted. Pressing Enter selects No by default.

```bash
sample=$(mktemp)
printf 'original\n' > "$sample"
cargo run --quiet --example decisions --features cli-confirmation -- replace "$sample"
cat "$sample"
```

The command reports `Kept original.`, exits successfully, and the file still reads `original`. Run the same command again and choose **Yes**. It reports `Replaced.`, and the file now reads `new contents`.

## Call without a terminal decision

Restore the original line. Ask for CLI JSON output, then read the file.

```bash
printf 'original\n' > "$sample"
cargo run --quiet --example decisions --features cli-confirmation -- replace "$sample" --json
cat "$sample"
```

The result contains `"Replacement requires explicit approval."` and the exit code is 1. The JSON view supplies no answer and the original contents survive. A caller that intends to replace the file can supply that decision in the input.

```bash
cargo run --quiet --example decisions --features cli-confirmation -- replace "$sample" --json --yes
```

This returns `"replaced"` in `result`. The action defines `yes` and its default of false. Trine supplies the decision channel and selects the view.

## Expose the same choice to an agent

Launch the example with `mcp` in place of `replace "$sample"`, then initialize the session as in the [first-action tutorial][first-action]. Send this tool call using the actual path printed by `echo "$sample"`.

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"replace","arguments":{"path":"/path/to/sample"}}}
```

The MCP binding supplies no interactive answer. The call returns a tool result with `isError: true`. With approval from the caller, send a fresh request ID and add `"yes":true` inside `arguments`. The same input works under RPC in `params.input`. Neither protocol needs a follow-up prompt exchange.

## Keep the policy in the handler

The human view implements `View::confirm` by delegating to `CliViewContext::confirm`. The optional `cli-confirmation` feature supplies terminal detection and prompt rendering. The handler awaits `context.confirm(question)` and owns the meaning of the response.

| Answer | What the handler does |
| --- | --- |
| `Some(true)` | Proceeds to the write. |
| `Some(false)` | Returns a successful `Kept` outcome. Declining is a completed decision. |
| `None` | Returns a domain error explaining that approval is required. The caller can retry with explicit approval. |

A decision request can also return an `EmitError`, for example when cancellation interrupts the wait. That is a separate path through the handler. Put effects after the decision so that a refusal, an unavailable prompt, or an interrupted wait can leave the file alone.

[decisions]: ../../examples/decisions.rs
[first-action]: first-action.md
