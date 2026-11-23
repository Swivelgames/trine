# Observe and cancel a directory scan

Suppose an editor wants to show filenames while it indexes a directory. A background job only needs the final count. Both can call the same `scan` action. The editor can also stop early and keep the count the scanner reached.

Run the [directory scan example][scan-example] from the Trine checkout. It counts regular files immediately inside the supplied directory and skips subdirectories and symlinks. It leaves the files untouched.

## Request just the count

Start the RPC host and leave it running.

```sh
cargo run --quiet --example directory_scan
```

Enter one request per line. The `directory` path is relative to the host's working directory. Here `src/view` contains the two source files for decision delivery and view erasure.

```json
{"jsonrpc":"2.0","id":1,"method":"invoke","params":{"action":["scan"],"input":{"directory":"src/view"}}}
```

The only response is the terminal result.

```json
{"jsonrpc":"2.0","id":1,"result":{"status":"complete","files":2}}
```

## Subscribe to filenames

In the same session, use a new ID and opt in to progress.

```json
{"jsonrpc":"2.0","id":2,"method":"invoke","params":{"action":["scan"],"input":{"directory":"src/view"},"observe":true}}
```

Each regular file produces a notification like this. The directory determines which filename appears first.

```json
{"jsonrpc":"2.0","method":"progress","params":{"id":2,"sequence":1,"event":"decision.rs"}}
```

Read until the terminal frame with top-level `id: 2`. Its count matches the first scan if the directory contents stayed the same. Progress notifications carry their invocation ID inside `params`; sequence numbers start at 1 for each observed invocation.

Follow the two choices in the source. The application registers `StructuredBinding::serde_progress(capacity)`, making filenames available to RPC callers. Each caller chooses whether to subscribe. With a subscriber, `context.emit(name).await` waits for space when the bounded queue fills. A successful send means the event was accepted for delivery. With observation disabled, emission returns immediately.

## Cancel a scan in flight

Use a directory with many regular files for this step. Send the following two frames on the **same connection**, replacing the directory before sending. To make interruption easier to observe, paste the frames together. An editor could instead send the second frame when its user clicks Stop.

```json
{"jsonrpc":"2.0","id":3,"method":"invoke","params":{"action":["scan"],"input":{"directory":"path/to/many/files"},"observe":true}}
{"jsonrpc":"2.0","method":"cancel","params":{"id":3}}
```

Keep reading. The cancellation notification has no response of its own. The invocation still returns one terminal result. Its `status` is `cancelled` when the handler observes the request, and `files` is the number it counted before stopping. Cancellation can arrive before the first file, or the scan may already have completed. Try omitting `observe` and cancelling again with a fresh ID in both frames. Cancellation also works for the background job.

Look at the loop in `scan`. It checks the cancellation token between entries, including entries it skips. It also handles cancellation during an awaited send. These two checks cover both a caller waiting quietly and a scanner waiting for a slow observer. The counter advances as soon as the scanner identifies a regular file, so its partial count can exceed the number of notifications the editor has received.

## Decide what belongs in the result

This example treats a requested stop as a usable partial result. An unreadable directory is an action failure. Try invoking `scan` on `Cargo.toml` with a fresh ID. The RPC response carries the filesystem diagnostic in `error.data`. A caller can distinguish a failed scan from a successful count of zero and from a requested stop before the first file. That distinction comes from the action's result type and error handling. Trine supplies the cancellation signal and delivers the outcome the action returns.

[scan-example]: ../../examples/directory_scan.rs
