//! Two typed actions with native inputs and independent CLI views. Simulation only.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{io, num::NonZeroUsize, time::Duration};
use trine::{
	Action, InvocationContext, NativePath,
	application::{Application, CliBinding, CliViewContext, Entry, Output},
	cli::{Alias, Bindings},
	structured::StructuredBinding,
	view::View,
};

#[derive(Deserialize, JsonSchema)]
struct CreateInput {
	task: String,
	repositories: Vec<Repository>,
	#[serde(default)]
	checkout: bool,
	#[serde(default)]
	no_fetch: bool,
	#[serde(default)]
	pull: bool,
	remote: Option<String>,
	from: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct Repository {
	path: NativePath,
	name: Option<String>,
	remote: Option<String>,
	from: Option<String>,
}

#[derive(Serialize)]
struct Created {
	task: String,
	completed: Vec<NativePath>,
	remaining: Vec<NativePath>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CreateOutcome {
	Completed {
		#[serde(flatten)]
		progress: Created,
	},
	Cancelled {
		#[serde(flatten)]
		progress: Created,
	},
}
#[derive(Serialize)]
struct CreateError {
	status: &'static str,
	#[serde(flatten)]
	progress: Created,
	error: ErrorDetail,
}
#[derive(Serialize)]
struct ErrorDetail {
	code: &'static str,
	message: String,
}

struct CreateProgress {
	path: NativePath,
	phase: &'static str,
}

async fn create(
	input: CreateInput,
	context: InvocationContext<CreateProgress>,
) -> Result<CreateOutcome, CreateError> {
	create_with_phase_wait(input, context, || async {
		smol::Timer::after(Duration::from_millis(100)).await;
	})
	.await
}

// The phase boundary is injectable so tests can hold an unfinished participant
// without depending on timer speed or view-worker scheduling.
async fn create_with_phase_wait<F: std::future::Future<Output = ()>>(
	input: CreateInput,
	context: InvocationContext<CreateProgress>,
	wait: impl Fn() -> F,
) -> Result<CreateOutcome, CreateError> {
	let mut outcome = Created {
		task: input.task,
		completed: Vec::new(),
		remaining: input
			.repositories
			.iter()
			.map(|repo| repo.path.clone())
			.collect(),
	};

	if outcome.task.is_empty() || input.repositories.is_empty() || (input.no_fetch && input.pull) {
		return Err(CreateError {
			status: "failed",
			progress: outcome,
			error: ErrorDetail {
				code: "invalid_input",
				message: "Supply a task and --repo; --no-fetch and --pull are mutually exclusive."
					.into(),
			},
		});
	}

	for repo in input.repositories {
		// Effective options are selected here, independently of presentation.
		let _effective_options = (
			repo.name,
			repo.remote.or(input.remote.clone()),
			repo.from.or(input.from.clone()),
			input.checkout,
		);

		let phases = [
			(!input.no_fetch, "fetch"),
			(true, "create worktree"),
			(input.pull, "pull"),
		];

		for (enabled, phase) in phases {
			if !enabled {
				continue;
			}

			if context.cancellation().is_cancelled() {
				return Ok(CreateOutcome::Cancelled { progress: outcome });
			}

			if context
				.emit(CreateProgress {
					path: repo.path.clone(),
					phase,
				})
				.await
				.is_err()
			{
				return Ok(CreateOutcome::Cancelled { progress: outcome });
			}

			// Simulate a cooperative phase while keeping cancellation responsive.
			let phase_finished = futures_lite::future::or(
				async {
					context.cancellation().cancelled().await;
					false
				},
				async {
					wait().await;
					true
				},
			)
			.await;

			if !phase_finished {
				return Ok(CreateOutcome::Cancelled { progress: outcome });
			}
		}

		// Preserve completed participants if a later phase is cancelled.
		outcome.completed.push(repo.path);
		outcome.remaining.remove(0);
	}

	Ok(CreateOutcome::Completed { progress: outcome })
}

#[derive(Deserialize, JsonSchema)]
struct ListInput {
	state_dir: NativePath,
}

#[derive(Serialize)]
struct Listed {
	state_dir: NativePath,
	tasks: Vec<String>,
}

struct ListProgress;

struct CreateView {
	output: CliViewContext,
}

impl View<CreateOutcome, CreateError, CreateProgress> for CreateView {
	type Completion = ();
	type Error = io::Error;

	async fn progress(&mut self, event: CreateProgress) -> io::Result<()> {
		// Debug formatting makes unusual path units visible without altering input.
		self.output
			.stderr
			.write_all(format!("{:?}: {}\n", event.path.as_path(), event.phase).as_bytes())
	}

	async fn finish(
		&mut self,
		result: &Result<CreateOutcome, CreateError>,
	) -> io::Result<Self::Completion> {
		let value = match result {
			Ok(outcome) => serde_json::to_value(outcome)?,
			Err(error) => serde_json::to_value(error)?,
		};
		let mut bytes = serde_json::to_vec(&value)?;
		bytes.push(b'\n');
		self.output.stdout.write_all(&bytes)?;
		Ok(())
	}
}

struct ListView {
	output: CliViewContext,
}

impl View<Listed, String, ListProgress> for ListView {
	type Completion = ();
	type Error = io::Error;

	async fn progress(&mut self, _: ListProgress) -> io::Result<()> {
		Ok(())
	}

	async fn finish(&mut self, result: &Result<Listed, String>) -> io::Result<()> {
		render_result(&self.output.stdout, result)
	}
}

fn render_result<O: Serialize, E: Serialize>(
	output: &Output,
	result: &Result<O, E>,
) -> io::Result<()> {
	let response = match result {
		Ok(value) => serde_json::json!({"status":"completed","outcome":value}),
		Err(error) => serde_json::json!({"status":"failed","error":error}),
	};

	let mut bytes = serde_json::to_vec(&response)?;
	bytes.push(b'\n');
	output.write_all(&bytes)
}

pub fn application() -> Result<Application, Box<dyn std::error::Error>> {
	application_with_create(Action::new("create", "Simulate task creation", create)?)
}

fn application_with_create(
	create_action: Action<CreateInput, CreateOutcome, CreateError, CreateProgress>,
) -> Result<Application, Box<dyn std::error::Error>> {
	let mut app = Application::new("workyard-demo", env!("CARGO_PKG_VERSION"))?;
	let create_binding = CliBinding::new(
		Bindings::new()
			.positional("task")
			.group("repositories", "path", "repo")
			.native_path("repositories[].path")
			.option("repositories[].name", "repo-name")
			.option("repositories[].remote", "repo-remote")
			.option("repositories[].from", "repo-from")
			.option("remote", "remote")
			.option("from", "from")
			.option("checkout", "checkout")
			.option("no_fetch", "no-fetch")
			.option("pull", "pull"),
	)
	.alias(Alias::new("checkout").set("checkout", true));
	app.register(
		Entry::new(create_action)
			.cli(
				create_binding,
				|output| Ok(CreateView { output }),
				|result| u8::from(!matches!(result, Ok(CreateOutcome::Completed { .. }))),
			)
			.rpc(StructuredBinding::serde_with_progress(
				|event: CreateProgress| {
					Ok(serde_json::json!({"path":event.path,"phase":event.phase}))
				},
				NonZeroUsize::new(16).unwrap(),
			))
			.mcp(StructuredBinding::serde()),
	)?;

	let list_action = Action::new(
		"list",
		"Simulate listing tasks",
		|input: ListInput, _: InvocationContext<ListProgress>| async {
			Ok::<_, String>(Listed {
				state_dir: input.state_dir,
				tasks: vec!["feat/payments".into()],
			})
		},
	)?;
	let list_binding = CliBinding::new(
		Bindings::new()
			.option("state_dir", "state-dir")
			.native_path("state_dir"),
	)
	.capacity(None);
	app.register(
		Entry::new(list_action)
			.cli(
				list_binding,
				|output| Ok(ListView { output }),
				|result| u8::from(result.is_err()),
			)
			.rpc(StructuredBinding::serde())
			.mcp(StructuredBinding::custom(|context| {
				Ok(ListMcpView { context })
			})),
	)?;
	Ok(app)
}

struct ListMcpView {
	context: trine::structured::StructuredContext,
}
impl View<Listed, String, ListProgress> for ListMcpView {
	type Completion = ();
	type Error = trine::error::Failure;
	async fn progress(&mut self, _: ListProgress) -> Result<(), trine::error::Failure> {
		Ok(())
	}
	async fn finish(
		&mut self,
		result: &Result<Listed, String>,
	) -> Result<(), trine::error::Failure> {
		use trine::structured::Payload;
		let payload = match result {
			Ok(list) => Payload::Success(
				serde_json::json!({"summary":format!("{} task(s)",list.tasks.len()),"tasks":list.tasks}),
			),
			Err(error) => Payload::DomainFailure(serde_json::json!({"message":error})),
		};
		self.context.terminal(payload).await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, Mutex};
	use trine::{Cancellation, cli::Parsed};

	#[derive(Clone, Default)]
	struct Capture(Arc<Mutex<Vec<u8>>>);
	impl io::Write for Capture {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			self.0.lock().unwrap().extend_from_slice(bytes);
			Ok(bytes.len())
		}
		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}

	struct CancelOnWeb(Cancellation);
	impl io::Write for CancelOnWeb {
		fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
			if String::from_utf8_lossy(bytes).contains("web") {
				self.0.cancel();
			}
			Ok(bytes.len())
		}
		fn flush(&mut self) -> io::Result<()> {
			Ok(())
		}
	}

	#[test]
	fn cancel_after_first_participant_is_flat_normal_outcome_with_nonzero_exit() {
		use std::sync::atomic::{AtomicUsize, Ordering};
		let action = Action::new("create", "Create", |input, context| async move {
			let phases = AtomicUsize::new(0);
			create_with_phase_wait(input, context, || {
				let first = phases.fetch_add(1, Ordering::SeqCst) == 0;
				async move {
					if !first {
						std::future::pending::<()>().await;
					}
				}
			})
			.await
		})
		.unwrap();
		let app = application_with_create(action).unwrap();
		let Parsed::Input(prepared) = app
			.parse_cli([
				"demo",
				"create",
				"feat/payments",
				"--repo",
				"api",
				"--repo",
				"web",
				"--no-fetch",
			])
			.unwrap()
		else {
			panic!("expected invocation")
		};
		let cancellation = Cancellation::new();
		let stdout = Capture::default();
		let report = prepared
			.run(
				CliViewContext {
					stdout: Output::new(stdout.clone()),
					stderr: Output::new(CancelOnWeb(cancellation.clone())),
				},
				cancellation,
			)
			.unwrap();
		assert!(report.domain_succeeded);
		assert!(report.presentation_succeeded);
		assert_eq!(report.exit_code(), 1);
		assert!(!report.diagnostics.attempted);
		assert_eq!(
			serde_json::from_slice::<serde_json::Value>(&stdout.0.lock().unwrap()).unwrap(),
			serde_json::json!({
				"status": "cancelled", "task": "feat/payments",
				"completed": ["api"], "remaining": ["web"]
			}),
		);
	}

	#[test]
	fn create_and_list_exit_policies_share_dispatch() {
		let app = application().unwrap();
		for args in [
			vec![
				"demo",
				"create",
				"feat/payments",
				"--repo",
				"api",
				"--no-fetch",
			],
			vec!["demo", "list", "--state-dir", ".workyard"],
		] {
			let Parsed::Input(prepared) = app.parse_cli(args).unwrap() else {
				panic!()
			};
			let report = prepared
				.run(
					CliViewContext {
						stdout: Output::new(Capture::default()),
						stderr: Output::new(Capture::default()),
					},
					Cancellation::new(),
				)
				.unwrap();
			assert_eq!(report.exit_code(), 0);
			assert!(!report.diagnostics.attempted);
		}
	}
}
