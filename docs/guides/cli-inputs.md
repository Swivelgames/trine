# Design CLI inputs that belong together

A repository selection often has two parts: a path and an optional remote. If a caller supplies three paths and one remote, which repository owns the remote? Give the selection a type, then use the path option to start each selection on the command line.

Run the preview from the Trine checkout. The command echoes the parsed selections, so you can experiment with paths before any repository exists.

```sh
cargo run --quiet --example repo-plan -- preview --repo api --remote origin --repo ../web
```

```text
api (origin)
../web
```

## Map words to typed fields

The example uses `Plan { repos: Vec<Repository> }`. Each `Repository` has a `NativePath` and an `Option<String>` remote. Add `--json` to the preview to inspect the rows inside `result.repos`. Their order follows the command line, and the second row has a null remote.

```rust
use trine::cli::Bindings;

let bindings = Bindings::new()
	.group("repos", "path", "repo")
	.native_path("repos[].path")
	.option("repos[].remote", "remote");
```

The three arguments to `group` are the collection, the field that opens a row, and the long option that supplies it. Each `--repo` appends a row and sets its path. The next `--remote` fills the remote in that row. An orphan `--remote` or a second remote in the same row gives a parse error before the handler runs. Try moving `--remote origin` to the end and compare the JSON.

Choose a positional for a value with an obvious place in the command. A list of strings or paths can use a repeated option or a final variadic positional. Use a repeated object group when each occurrence owns fields of its own. The schema-assisted CLI accepts one such group per input. See the [example registration][repo-plan].

## Keep paths native

Pass `std::env::args_os()` into Trine. Pair the `NativePath` field with `.native_path("repos[].path")` and pass its native value to filesystem or process APIs. The human view uses `.display()` at the point of rendering. That display string can lose information and should stay out of the action input.

Unicode paths serialize as JSON strings. Other Unix paths serialize as a tagged object containing base64 bytes, and other Windows paths as a tagged object containing UTF-16 code units. The same host can deserialize that representation back to the original path. Shell completion scripts have their own output encoding limits, so a path accepted as input may still need to be entered manually.

## Complete the remote at the cursor

Register a provider on the child field with `.complete("repos[].remote", remotes::suggest)`. The callback receives the parsed cursor context and the prefix being edited.

1. Ask `context.current_group("repos")` for the row that owns the cursor. A later `--repo` may already be present if the user moves back to edit an earlier argument.
2. Read `row.field("path")` and proceed only for `OptionValue::Present(path)`. Partial or ambiguous input is ordinary while typing. Return an empty list when the row cannot be determined.
3. Resolve the path against `context.cwd` and run `git -C <path> remote`. Return the names matching the prefix. This reads Git configuration locally. The preview accepts an explicitly typed remote even if Git is unavailable or no candidate matches.

A plain provider returns insertion values. For values that need explanation, `complete_candidates` accepts a group label and candidates with optional descriptions. The shell decides how to display those descriptions. See the [remote provider][remotes].

## Try it in Bash

From the Trine checkout, build the example and make its executable available under the name in `Application::new`. The generated completion function invokes that name when you press Tab.

```bash
cargo build --quiet --example repo-plan --target-dir target
export PATH="$PWD/target/debug/examples:$PATH"
source <(repo-plan completion bash)

sandbox=$(mktemp -d)
git init -q "$sandbox/api"
git init -q "$sandbox/web"
git -C "$sandbox/api" remote add origin .
git -C "$sandbox/web" remote add upstream .
cd "$sandbox"
```

Type `repo-plan preview --repo api --remote o` and press Tab. Bash completes `origin`. Append ` --repo web --remote u` and press Tab again for `upstream`. Move back to the first remote and complete it again: its suggestions still come from `api`. The two scratch repositories contain only local configuration.

In the executable, handle `completion::requested()` before normal CLI parsing. Pass the native arguments and working directory to `completion::respond` and write the returned bytes to stdout. Both that query and `completion::generate` use `app.cli_command()`, keeping the provider attached to the same field the ordinary parser accepts.

[repo-plan]: ../../examples/repo-plan/main.rs
[remotes]: ../../examples/repo-plan/remotes.rs
