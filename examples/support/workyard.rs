//! Workyard-shaped domain simulation. No Git, paths, or workspace state are touched.

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trine::{
	Action, InvocationContext,
	cli::{Alias, Bindings, Cli},
};

/// Shared input for direct calls and CLI invocations.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct CreateInput {
	pub task: String,
	pub repositories: Vec<RepositoryInput>,
	pub tasks_root: Option<String>,
	pub state_dir: Option<String>,
	pub from: Option<String>,
	pub remote: Option<String>,

	#[serde(default)]
	pub checkout: bool,
	#[serde(default)]
	pub no_fetch: bool,
	#[serde(default)]
	pub pull: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct RepositoryInput {
	pub path: String,
	pub name: Option<String>,
	pub remote: Option<String>,
	pub from: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Outcome {
	pub task: String,
	pub completed: Vec<String>,
	pub remaining: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Failure {
	Invalid { message: String, remedy: String },
	Cancelled { outcome: Outcome, remedy: String },
}

#[derive(Debug)]
pub struct Progress {
	pub repository: String,
	pub phase: &'static str,
}

pub type CreateAction = Action<CreateInput, Outcome, Failure, Progress>;
pub type CreateCli = Cli<CreateInput, Outcome, Failure, Progress>;

/// Register the simulation handler once, independently of CLI presentation.
pub fn action(step_duration: Duration) -> CreateAction {
	Action::new(
		"create",
		"Simulate creating a task across repository worktrees",
		move |input, context| simulate(input, context, step_duration),
	)
	.expect("valid example action")
}

/// Bind CLI spellings to input fields; each `--repo` starts a repository entry.
pub fn cli(action: CreateAction) -> CreateCli {
	Cli::new(
		"workyard-demo",
		env!("CARGO_PKG_VERSION"),
		action,
		Bindings::new()
			.positional("task")
			.group("repositories", "path", "repo")
			.option("repositories[].name", "repo-name")
			.option("repositories[].remote", "repo-remote")
			.option("repositories[].from", "repo-from")
			.option("tasks_root", "tasks-root")
			.option("state_dir", "state-dir")
			.option("from", "from")
			.option("remote", "remote")
			.option("checkout", "checkout")
			.option("no_fetch", "no-fetch")
			.option("pull", "pull"),
		vec![Alias::new("checkout").set("checkout", true)],
	)
	.expect("valid example CLI bindings")
}

/// Both CLI and direct callers enter the same domain validation and handler.
pub async fn simulate(
	input: CreateInput,
	context: InvocationContext<Progress>,
	step_duration: Duration,
) -> Result<Outcome, Failure> {
	validate(&input)?;

	let mut outcome = Outcome {
		task: input.task.clone(),
		completed: Vec::new(),
		remaining: input
			.repositories
			.iter()
			.map(|repo| repo.path.clone())
			.collect(),
	};

	for repo in &input.repositories {
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
				return Err(cancelled(outcome));
			}

			if context
				.emit(Progress {
					repository: repo.path.clone(),
					phase,
				})
				.await
				.is_err()
			{
				return Err(cancelled(outcome));
			}

			// The delay makes live progress visible. It models a cooperative phase,
			// not a timeout or a claim about how real Git processes are cancelled.
			let phase_finished = futures_lite::future::or(
				async {
					context.cancellation().cancelled().await;
					false
				},
				async {
					smol::Timer::after(step_duration).await;
					true
				},
			)
			.await;

			if !phase_finished {
				return Err(cancelled(outcome));
			}
		}

		// Record completion before reporting it, so cancellation preserves this work.
		outcome.completed.push(repo.path.clone());
		outcome.remaining.remove(0);

		if context
			.emit(Progress {
				repository: repo.path.clone(),
				phase: "complete",
			})
			.await
			.is_err()
		{
			return Err(cancelled(outcome));
		}
	}

	Ok(outcome)
}

fn validate(input: &CreateInput) -> Result<(), Failure> {
	let problem = if input.no_fetch && input.pull {
		Some("--no-fetch and --pull are mutually exclusive")
	} else if input.task.trim().is_empty() {
		Some("task must not be empty")
	} else if input.repositories.is_empty()
		|| input
			.repositories
			.iter()
			.any(|repo| repo.path.trim().is_empty())
	{
		Some("supply at least one nonempty --repo identifier")
	} else {
		None
	};

	if let Some(message) = problem {
		return Err(Failure::Invalid {
			message: message.into(),
			remedy: "Adjust the arguments and rerun this simulation; no workspace was changed."
				.into(),
		});
	}

	Ok(())
}

fn cancelled(outcome: Outcome) -> Failure {
	let remedy = "This was a simulation; completed steps are reported only. \
		Rerun when ready. No workspace needs recovery.";
	Failure::Cancelled {
		outcome,
		remedy: remedy.into(),
	}
}
