# Trine

Trine is an experimental, unpublished Rust library for sharing typed actions
between interfaces. It provides direct async invocation, a schema-assisted CLI,
typed registry entries, independent views and native-path inputs. A live stdio RPC host is available; MCP remains an in-process harness.

An action owns its input, output, domain error and progress types. The CLI owns
argument parsing, runtime startup and presentation. Trine does not know about Git,
worktrees, filesystem transactions or Workyard state.

## Try the simulation

```sh
cargo run --example workyard_create -- --help
cargo run --example workyard_create -- create feat/payments \
	--remote origin --repo api --repo-name backend --repo-remote upstream \
	--repo web --repo-from main --pull
cargo run --example workyard_create -- checkout feat/payments --repo api
```

This only simulates sequential phases, with short delays so progress is visible.
Progress goes to stderr; the terminal JSON result goes to stdout. Ctrl+C requests
cooperative cancellation and returns a nonzero exit status with completed and
remaining participants. Validation errors also exit nonzero. No real repositories
or workspace state are read or changed. `--no-fetch` and `--pull` are mutually
exclusive in the shared example handler.

Repository overrides bind to the nearest preceding `--repo`. Common `--remote`
and `--from` stay at the root regardless of position. `checkout` is the same
action with `checkout = true`. Equals syntax works (`--repo=api`); boolean flags
accept an optional explicit equals value (`--pull=false`). Duplicate scalar values
and overrides before their group anchor are errors.

## Author an action

```rust
use std::num::NonZeroUsize;
use schemars::JsonSchema;
use serde::Deserialize;
use trine::{Action, Cancellation, InvocationContext};
use trine::cli::{Bindings, Cli, Parsed};

#[derive(Deserialize, JsonSchema)]
struct Greeting {
	name: String,
}

let action = Action::new(
	"greet",
	"Greet someone",
	|input: Greeting, context: InvocationContext<String>| async move {
		let _ = context.emit("Preparing greeting".into()).await;
		Ok::<_, String>(format!("Hello, {}!", input.name))
	},
)?;

// Direct callers provide their own executor and invocation context.
let direct_action = action.clone();
let direct = smol::block_on(direct_action.invoke(
	Greeting { name: "Ada".into() },
	InvocationContext::unobserved(Cancellation::new()),
));
assert_eq!(direct.unwrap(), "Hello, Ada!");

let cli = Cli::new(
	"example", "0.1.0", action,
	Bindings::new().positional("name"),
	vec![],
)?;
match cli.parse(["example", "greet", "Ada"])? {
	Parsed::Display(help_or_version) => print!("{help_or_version}"),
	Parsed::Input(input) => {
		let result = cli.run(
			input, NonZeroUsize::new(16).unwrap(), Cancellation::new(),
			|event| eprintln!("{event}"),
		);
		println!("{}", result.unwrap());
	}
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

The example executable installs Ctrl+C handling **after** parsing and passes the
same cancellation handle to `Cli::run`. The library does not install process-global
signal handlers. Help/version finish before any runtime or handler starts.

## Binding and schema boundaries

`Action::new` derives a Schemars input schema. `Cli::new` validates the supported
schema subset and all explicit bindings before constructing Clap commands:

- One input object, with string/boolean/native-path properties and nullable
	versions of those scalars. Local references emitted by derived schemas are resolved.
- At most one repeated object group; its child properties use the same scalar
	subset. `Bindings::group("repositories", "path", "repo")` declares the anchor.
	`Bindings::option("repositories[].remote", "repo-remote")` declares an override.
- At most one string positional. Other properties have explicit long-option
	spellings. Every property must be bound exactly once.
- Aliases invoke the same action and can assign root scalar values. Duplicate or
	reserved command names, duplicate options and incompatible assignments fail
	during registration.

Nested objects, numeric fields, arbitrary maps, enums and more complex unions
are rejected with the affected field named. This is not a general JSON Schema
validator: domain constraints belong in the shared handler. Serde determines
required fields and defaults; parsing does not inject separate default values.

`Cli::custom` is the escape hatch. Its normalization callback receives untouched
`Vec<OsString>` arguments, including argv0, and returns typed input or help/version
text. This supports custom argument layouts without coupling Clap types to
actions. Text fields reject invalid Unicode; explicitly bound native paths
preserve their OS representation. Neither silently substitutes replacement characters.

## Progress and cancellation

`InvocationContext::observed` creates an invocation-scoped bounded FIFO.
`emit().await` waits for space; it never drops or coalesces events. Cancellation
interrupts a blocked send and has priority if cancellation and capacity become
ready together. There is no automatic timeout. Without an observer, emission is
a no-op, and the handler checks cancellation separately at safe boundaries.

A cancelled action is still awaited. The handler chooses its domain result and
preserves completed work; Trine does not invent recovery or rollback semantics.
No handler is spawned and detached by this API. Runtime-independent action
contracts expose standard futures; the CLI runner uses Smol internally.

Action completion closes all emitter clones. Accepted events remain available to
drain, and `Cli::run` drains them before returning the terminal result. A renderer
runs synchronously on the CLI thread: keep it responsive, since slow output
naturally applies backpressure. Background emitters are not an extension of the
action lifetime. Create a fresh context for each invocation.

## Development

```sh
export CARGO_TARGET_DIR="$PWD/target"
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo tree --edges normal
python3 tests/smoke_example.py
python3 tests/smoke_registry.py
```

`.cargo/config.toml` defaults build output to this checkout's `target/`. An
inherited `CARGO_TARGET_DIR` overrides that configuration; the development
commands above explicitly select this checkout. The smoke harness also sets its
checkout-local target directory. Action APIs are experimental. This is one library
crate with typed actions, registry dispatch, and views. Generated builders,
MCP transport and publication are deferred. Normal dependencies use Smol
and narrow Clap features, with no Tokio, HTTP or OAuth stack. The signal-handling
crate is example/test-only.

## Registry, independent views, and native paths

The second example assembles two differently typed actions before erasing their
registry entries:

```sh
export CARGO_TARGET_DIR="$PWD/target"
cargo run --example workyard_registry -- --help
cargo run --example workyard_registry -- create feat/payments --repo api --repo web
cargo run --example workyard_registry -- checkout feat/payments --repo api
cargo run --example workyard_registry -- list --state-dir .workyard
```

`Registry::register(path, cli, view_factory, capacity)` checks every command and
alias before publishing the entry. Paths are segments such as `["task", "create"]`;
aliases share the parent's namespace. Duplicate routes and leaf/namespace prefix
conflicts fail atomically. `Registry::parse` produces help text or a prepared
invocation; parsing constructs no view and runs no handler. Each prepared run
gets a fresh view. This slice registers schema-assisted CLIs; the standalone
`Cli::custom` escape hatch remains available outside the registry.

`View<Output, Error, Event>` has async `progress` and `finish` methods. `view::run`
drives the action on the calling thread and the view on a joined worker with its
own Smol runner. The bounded FIFO absorbs bursts while rendering is busy; once
full, emission waits. Set capacity to `None` for terminal-only presentation.
The earlier `Cli::run` callback convenience method remains available with its
original synchronous callback scheduling.

A progress error or renderer panic closes delivery and requests cancellation.
The action is still awaited, and the host retains its typed outcome separately
from `PresentationError`. A failed progress view receives no terminal call.
Healthy delivery drains accepted events before one terminal attempt. `finish`
borrows the outcome, so its failure cannot consume the host's result. Outputs
and domain errors must be `Send + Sync` for this host; events must be `Send`.
`DispatchReport::typed` retrieves a known entry's typed report after dispatch.
An arbitrary blocking writer can delay worker joining indefinitely; there is no
automatic timeout, forced thread termination, or detached cleanup. Panics under
Rust's unwind strategy release channels; process-aborting panics cannot recover.

`NativePath` wraps a host path and rejects embedded NUL. Unicode paths serialize
as ordinary JSON strings. Other Unix paths use
`{"platform":"unix","bytes":"<base64>"}`; other Windows paths use
`{"platform":"windows","wide":[<u16 units>]}`. The decoder rejects malformed,
unknown-field, and foreign-platform encodings. Conversion does not check the
filesystem or normalize paths. `resolve(cwd)` joins against an explicit invocation
working directory without changing process CWD.

Native CLI fields need `.native_path("repositories[].path")` binding metadata.
`Cli::new` deserializes directly into the action's existing input type, including
`NativePath`, optional paths and ordered repository objects. Serde retains control
of required fields, defaults, renamed fields and unknown-field policy. No duplicate
scalar structs or manual path reconstruction are needed. Path arguments never
pass through JSON strings or the structured wire codec.

`Cli::with_input` remains available for custom assembly. Its callback receives
`InputFields`: ordinary Serde-compatible scalars, native paths and ordered group
rows. Call `fields.deserialize()` for normal typed construction, or extract selected
paths/groups before deserializing the remaining fields. Ambiguous field names across
those representations are rejected. Errors identify the affected field and group row.
This bridge covers the supported CLI shapes above; it is not a general-purpose Serde
format. Serde shapes that buffer native values, such as `#[serde(flatten)]`, cannot
use direct native deserialization, even when their derived schema is accepted at
registration. Parsing reports this limitation before invoking the handler. Use
`Cli::with_input`, extract native fields with `InputFields::take_path`, and assemble
the containing input explicitly. This limitation does not change the JSON wire codec.
The structured codec is used by live RPC; MCP transport remains future work.

## Shared application catalog

`Application` registers each action once, then projects its enabled interfaces.
The experimental authoring shape is:

```rust,ignore
let mut app = Application::new("workyard", env!("CARGO_PKG_VERSION"))?;
app.register(
	Entry::new(create_action)
		.cli(create_binding, |context| Ok(CreateCliView::new(context)), create_exit_code)
		.rpc(StructuredBinding::serde())
		.mcp(StructuredBinding::custom(CreateMcpView::new)),
)?;
```

`CliBinding` contains the argument bindings, aliases and optional progress capacity.
`Entry::namespace(["task"])` places the existing action name under a namespace.
`cli_with_input` retains custom native input assembly. `parse_cli` constructs typed
input without executing handlers or view factories. `PreparedCli::run` receives a
fresh `CliViewContext` with replaceable stdout/stderr handles; factory failures are
setup errors and do not start the action. The initial CLI context carries output
handles only; color/format preferences can be added when a consumer requires them.

Registration validates the full entry before publishing routes. Discovery and
invocation expose only interfaces explicitly attached to that entry. Repeated
interface bindings, duplicate actions and conflicting CLI aliases/namespaces fail.
The existing standalone `Cli`, `Registry` and `view::run` APIs remain available.

`invoke_rpc` and `invoke_mcp` are **in-process harness entrypoints**, not protocol
servers. They decode JSON into the same action input and select their registered
view. `StructuredContext` is an invocation-scoped content sink. `Payload::Success`
and `Payload::DomainFailure` are internal presentation values, not RPC/MCP envelopes.
These harness entrypoints perform no framing or remote interaction. Both synchronous
conveniences drive the prepared async execution lifecycle; `rpc::serve` adds live transport.

Standard `StructuredBinding::serde()` views require serializable outcomes/errors,
but no event serialization. They default to terminal-only presentation.
`serde_with_progress(mapper, capacity)` adds explicit event mapping; an invocation
must also set `observe = true`. Custom views opt into progress with `.observe(capacity)`.
CLI registrations require serializable output and domain errors, but progress needs no
serialization. RPC registration remains independent. All view factories
are fallible and fresh per invocation; custom view errors retain classified causes in
the erased report, while the domain outcome remains typed and available through
`DispatchReport::typed::<Output, DomainError, Failure>()`.

Structured terminal submission is allowed only during `finish`. Zero or duplicate
terminal payloads produce presentation errors, and a duplicate cannot replace the
first payload. Retained sinks close when the invocation ends, including unwind.
Progress delivery still uses the bounded FIFO; the harness records delivered JSON
in an in-memory transcript for inspection, so transcript size itself is not bounded.
This capture is intended for small experiments, not production transport buffering.

The two examples share `examples/support/shared_workyard.rs`, defining create/list
and their registrations once. Try the structured projection with:

```sh
export CARGO_TARGET_DIR="$PWD/target"
cargo run --example shared_registration
python3 tests/smoke_shared.py
```

### Prepared async execution

`Application::prepare_rpc` and `prepare_mcp` decode input and construct the
selected view before returning. The returned invocation exposes `cancellation()`
and consumes itself with `run()`, yielding an ordinary Send future. The host
schedules and retains that future through settlement, even after disconnect.
Calling cancellation requests cooperation; dropping the future is not settlement.
Core coordination starts no executor or detached worker.

`PreparedCli::prepare(context, cancellation)` similarly initializes a fresh view.
Its blocking runner retains independent, joined view-worker isolation. The
existing CLI `run(context, cancellation)` convenience composes these steps.
An indefinitely blocked writer can still delay joining.

Reports now contain `ExecutionOutcome::Completed(Result<Output, DomainError>)`
or `Failed(ExecutionFailure)`. Presentation status remains independent. A caught
handler panic drains healthy accepted progress but does not call typed `finish`.
The application hosts supply framework diagnostics for both CLI and structured
invocations; `execution_failure` remains available for inspection. Neither result
implies external effects were rolled back.

Shared views now preserve `Failure` classifications and underlying error sources.
Custom view errors convertible to a boxed standard error retain their concrete
cause; existing strings can explicitly provide message-only errors. Structured
factory and progress-mapper APIs return `Failure`. Native CLI inputs and typed
action handlers are unchanged. The harness's framework payload is experimental,
not an RPC/MCP envelope or proof of client receipt. See `tests/async_host.rs` for
single-executor cancellation under bounded queue pressure.

### View completion and host diagnostics

Every `View` now declares `type Completion`, and `finish` returns that value.
Entry CLI human views return `()`; their registration supplies the shared exit
policy for human and JSON presentation. Custom structured views also return unit.
Lower-level runners retain arbitrary completion types, including `CliCompletion`.
Reports use `PresentationState::Completed(value)`, `Failed(error)` or
`Skipped`. Skipped means abnormal execution supplied no domain result for finish;
accepted progress may already have been presented.

`DispatchReport::exit_code()` honors CLI completion unless execution or presentation
failed, which always selects a nonzero exit. Unit completion defaults to zero for
a returned Ok and one for a returned Err. The shared create simulation returns
cancellation as a normal typed outcome but its registered exit policy selects exit one:

```json
{"status":"cancelled","task":"feat/payments","completed":["api"],"remaining":["web"]}
```

Actual create errors instead carry `status: "failed"` and an `error` object with
code/message. Neither action result is wrapped in a second contradictory status.

Generic hosts inspect `presentation_status`, `presentation_failure()` and
`presentation_cause()` without knowing action types. Concrete error sources survive.
`typed_completion::<Output, DomainError, Failure, CliCompletion>()` retrieves the
shared CLI report; `typed::<Output, DomainError, Failure>()` remains the unit form.
The standalone Registry now requires standard error types for generic cause access.

`Application::diagnostic_renderer(interface, DiagnosticRenderer::new(...))`
customizes framework text once per interface. Renderers are synchronous and should
return promptly. Domain errors still go to typed finish. Framework diagnostics
run after settlement, preserve original failures and use one bounded fallback if
rendering or delivery fails/panics. `report.diagnostics` records those attempts
separately. CLI diagnostics use stderr; structured presentation failures appear in
`transcript.diagnostics` without replacing an accepted terminal. Abnormal execution
with healthy presentation gets one framework terminal, while typed presentation
remains Skipped. These are harness semantics, not RPC/MCP protocol guarantees.

## Live stdio RPC

`cargo run --example workyard_rpc` serves the shared create/list simulation over
newline-delimited JSON-RPC. It performs no Git or workspace effects. Send an
invocation while keeping stdin open to observe progress before its final response:

```json
{"jsonrpc":"2.0","id":"example-1","method":"invoke","params":{"action":["create"],"input":{"task":"feat/payments","repositories":[{"path":"api"},{"path":"web"}]},"observe":true}}
```

`rpc::serve` accepts asynchronous input/output, configuration, a shutdown token and
a settlement observer. The host owns its invocation futures and serialized writer
through settlement. The observer can inspect `DispatchReport` and typed outcomes
without retaining an unbounded connection-wide report collection. The executable
installs signal handling explicitly. `rpc::stdio::open` provides exclusive native
stdio adapters: Unix readiness with restored descriptor flags, Windows bounded
workers with synchronous I/O cancellation and joined shutdown. Other asynchronous
transports supplied to `serve` must support safe cancellation of pending I/O.

RPC uses the same registered actions and view factories as the in-process harness.
`StructuredContext::progress` and `terminal` are now async: custom views await both
methods. The live backend applies bounded backpressure; the explicitly selected
harness still records a transcript. `Payload::RpcError` lets custom RPC views supply
an application code, message and data; reserved protocol codes are rejected.

The client sends `cancel` notifications with `params.id`. Optional server progress
contains `params.id`, `sequence` (starting at one per invocation) and view-owned
`event`. Normal cancellation remains an action outcome. Duplicate active/recent IDs
produce `rejected` notifications with `params.code = "duplicate_id"` and
`params.data.id`; the original invocation retains its sole correlated response.
Clients should tolerate unknown notifications and generate unique IDs.

Default limits are 16 active invocations, 4096 recently settled IDs, 256 UTF-8 bytes
per string ID, and 1 MiB per input/output frame. Ordinary output is bounded by 64
messages and 4 MiB; rejections have separate 16-message/64-KiB capacity. Settings are
host-configurable. At invocation capacity, requests receive `busy`; no waiting
invocation queue is created. Rejection overflow closes delivery, cancels and settles
owned work. No automatic timeout is applied.

Clean input EOF drains work and output. Read failure requests cancellation while
retaining usable output; write failure closes delivery and cancels work. Oversized
input is discarded through newline before parsing resumes. Oversized output fails
presentation and preserves the action outcome, with a small framework response
when possible. Batch arrays are rejected without executing members. This first RPC
profile has no discovery, initialization, replay, acknowledgments or MCP server.

Run `python3 tests/smoke_rpc.py` for real subprocess observations. Native runtime
verification recorded for this slice is Linux x86_64; Windows/macOS/ARM64 require
native execution before claiming equivalent platform evidence.

## Source organization

Public modules act as stable facades over focused implementation modules. Moving
an implementation does not require callers to change their imports.

| Responsibility | Owner |
| --- | --- |
| Action catalog, discovery and interface selection | `application.rs` |
| Typed entry and interface assembly | `application/entry.rs`, with CLI and structured assembly beneath `application/entry/` |
| Invocation-scoped terminal output facilities | `application/presentation.rs` |
| Generic typed-to-erased view adaptation | `view/erased.rs` |
| Structured payload contracts | `structured/payload.rs` |
| Structured invocation lifetime and output coordination | `structured/context.rs` |
| Structured factory, default serialization and terminal guard | Focused modules under `structured/` |
| CLI registry routing and retained dispatch reports | `registry.rs` and `registry/report.rs`, respectively |
| RPC connection lifecycle and serialized output delivery | `rpc/host.rs` and `rpc/host/writer.rs`, respectively |

Keep each capability's assembly with that capability. Keep small supporting types
near the top of their owner's file; give substantial supporting behavior a focused
submodule. Preserve existing public re-exports deliberately. These boundaries do
not require one file per type, a fixed directory depth or additional crates. For
example, the outbox's small queue and state helpers remain beside the outbox they
support, and schema helpers remain together as one cohesive translation concern.

### Terminal confirmations and choices

Enable `cli-confirmation` to use `CliViewContext::confirm` from a CLI view's
`confirm` callback. The helper uses Inquire with only its Crossterm backend enabled,
reads the real terminal input and renders prompts on real stderr. It is available
only when stdin, stderr and the view's stderr capability are terminal-enabled;
redirected stdout stays free of prompt output. Workyard-style application policy
still decides which questions to ask and what unsupported interaction means.

Ctrl+C maps to invocation cancellation after Inquire restores terminal modes.
Escape and Ctrl+D decline, and an EOF never grants consent. The synchronous prompt
runs on the joined presentation worker: external token cancellation does not wake
an active terminal read automatically. No reader thread is abandoned. Structured
invocations do not subscribe to confirmation requests and return `None` immediately.

For a menu, call `InvocationContext::select` with a `Selection` containing a
message and ordered option labels. A CLI view can delegate its `select` callback
to `CliViewContext::select` under the same Cargo feature. The answer is the
zero-based index of the chosen label. Include a Cancel item when the action
offers cancellation. Esc, Ctrl+C, and input EOF cancel the terminal selection.
A view without selection support returns `None`. Actions can use their typed
outcome to report any decision an unattended caller still needs to make.

Before presenting either prompt, the CLI view driver completes delivery of progress
already accepted by the invocation. Applications can disclose facts once through
progress and ask a short question without repeating those facts inside the prompt.

## Direct CLI JSON and shell completions

`Entry::cli(binding, human_factory, exit_policy)` always supports `--json`. Its
output and domain-error types must implement `Serialize`. The typed
exit-policy callback receives `&Result<Output, Error>` and returns a `u8`; it is
shared by human and JSON presentations. Human views return `()` and focus on
rendering. Use `cli_with_input` when custom native input assembly is needed.

JSON mode does not construct the human view. It disables progress and interactive
confirmation, retains the action's typed result, and writes one four-space-indented
JSON document after execution settles:

```json
{
    "exit_code": 0,
    "result": { "message": "Hello" },
    "error": null
}
```

Domain errors and cancellation remain in `result`. The `error` field is reserved
for failures of parsing, execution or presentation, with stable `code` and readable
`message` fields. A framework failure can accompany an available result. Results
are encoded before writing; a failing stdout produces a nonzero exit and a stderr
diagnostic rather than a second JSON document. RPC envelopes are unchanged.

Hosts can use `cli::output_mode::json_requested` and
`application::json::JsonResponse::failure` to render errors that occur before a
prepared invocation exists. The mode detector respects `--` and does not interpret
`--option=--json` values as flags. Explicit help/version remain ordinary displays.
`--json` is reserved for every Entry CLI action; conflicting action bindings are
rejected during registration. Progress types do not require serialization.

`Application::cli_command()` returns the registered Clap command tree, including
aliases, namespaces and JSON support. Hosts may add their own commands to that
projection and call `cli::completion::generate(&mut command, shell)`. Generation
returns script bytes without writing files or modifying shell configuration.
Native-path bindings retain filesystem suggestions. Register domain suggestions with
`Bindings::complete("field", provider)`. Providers receive `completion::Context`
(native partial arguments, cursor index and working directory) plus the current
value. They return candidate `OsString`s without constructing an action or view.
Use `Bindings::complete_group("field", "Tasks", provider)` to give suggestions a
semantic label. Zsh displays named domain groups before options and retains existing
help descriptions. Its normal completion styles control formatting; absent a format
style, headings remain visible. Candidate values are separate from display text.

Repeated-group child options become suggestions only after a complete anchor value
before the cursor, using the declared binding relationship rather than flag prefixes.
This advisory filtering does not change action parsing or validate repository paths.

Construct raw input with `Context::new(arguments, index, cwd)`. Before invoking
providers, Trine derives partial input once from bindings and alias assignments.
During callbacks, `Context::field` and `Context::option` distinguish absent,
incomplete, supplied and ambiguous values, including arguments after the cursor.
`Context::groups` exposes explicitly supplied rows and `Context::current_group`
selects the row whose anchor precedes the cursor. `Group::field` preserves native
values and per-row ambiguity. No action input is admitted or deserialized here.
An empty provider result never enables file fallback.

Hosts check `completion::requested()` before normal parsing, then call
`completion::respond(command, arguments, cwd)`. Completion failures should return
no candidates without diagnostics. Query transport is private to generated scripts;
reload scripts after upgrading. Bash/Zsh/Fish/PowerShell registrations are narrowly
owned adapters around the pinned clap_complete dynamic engine. They never evaluate
command-line text. Bash uses a literal tokenizer to preserve values across Readline
word breaks; unresolved shell expansions yield no suggestions. Candidates that the
shell line protocol cannot represent faithfully are omitted, not converted lossily.
Native input paths remain intact. Bash versions without `compopt` retain no-space
continuation for all candidates; users add the separating space themselves. Fish
and PowerShell native execution still require platform acceptance; Rust transport
coverage does not establish shell execution.
