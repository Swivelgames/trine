//! Register the simulated create action, its checkout alias and its interface views.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{io, num::NonZeroUsize, time::Duration};
use trine::{
	Action, Application, Entry, InvocationContext, NativePath, RegistrationError,
	application::{CliBinding, CliViewContext},
	cli::{Alias, Bindings},
	structured::StructuredBinding,
	view::View,
};

pub(super) fn register(app: &mut Application) -> Result<(), RegistrationError> {
	let action = Action::new("create", "Simulate task creation", create)?;
	app.register(bind(action))
}

fn bind(
	action: Action<CreateInput, CreateOutcome, CreateError, CreateProgress>,
) -> Entry<CreateInput, CreateOutcome, CreateError, CreateProgress> {
	let bindings = Bindings::new()
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
		.option("pull", "pull");
	let cli = CliBinding::new(bindings).alias(Alias::new("checkout").set("checkout", true));

	let rpc = StructuredBinding::serde_with_progress(
		|event: CreateProgress| Ok(serde_json::json!({"path":event.path,"phase":event.phase})),
		NonZeroUsize::new(16).unwrap(),
	);

	Entry::new(action)
		.cli(
			cli,
			|output| Ok(CreateView { output }),
			|result| u8::from(!matches!(result, Ok(CreateOutcome::Completed { .. }))),
		)
		.rpc(rpc)
		.mcp(StructuredBinding::serde())
}

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

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, Mutex};
	use trine::{Application, Cancellation, application::Output, cli::Parsed};

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
		let mut app = Application::new("demo", "1").unwrap();
		app.register(bind(action)).unwrap();
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
}
