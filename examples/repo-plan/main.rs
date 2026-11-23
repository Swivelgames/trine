//! Preview grouped repository arguments and complete a remote for the current row.

mod remotes;

use std::{
	env,
	error::Error,
	io::{self, Write},
	process::ExitCode,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, NativePath,
	application::{CliBinding, CliViewContext},
	cli::{Bindings, Parsed, completion},
	view::View,
};

#[derive(Deserialize, JsonSchema, Serialize)]
struct Repository {
	/// Path to a repository. Repeat --repo to start another selection.
	path: NativePath,
	/// Remote for the preceding repository.
	remote: Option<String>,
}

#[derive(Deserialize, JsonSchema, Serialize)]
struct Plan {
	repos: Vec<Repository>,
}

struct PlanView(CliViewContext);

impl View<Plan, String, ()> for PlanView {
	type Error = io::Error;
	type Completion = ();

	async fn finish(&mut self, result: &Result<Plan, String>) -> io::Result<()> {
		let plan = match result {
			Ok(plan) => plan,
			Err(message) => return self.0.stderr.write_all(format!("{message}\n").as_bytes()),
		};

		for repo in &plan.repos {
			let path = repo.path.as_path().display();
			let line = match &repo.remote {
				Some(remote) => format!("{path} ({remote})\n"),
				None => format!("{path}\n"),
			};
			self.0.stdout.write_all(line.as_bytes())?;
		}
		Ok(())
	}
}

fn application() -> Result<Application, Box<dyn Error>> {
	let bindings = Bindings::new()
		.group("repos", "path", "repo")
		.native_path("repos[].path")
		.option("repos[].remote", "remote")
		.complete("repos[].remote", remotes::suggest);

	let action = Action::new(
		"preview",
		"Show the selected repositories",
		|plan: Plan, _: InvocationContext<()>| async move { Ok::<_, String>(plan) },
	)?;
	let entry = Entry::new(action).cli(
		CliBinding::new(bindings),
		|context| Ok(PlanView(context)),
		|result| u8::from(result.is_err()),
	);

	let mut app = Application::new("repo-plan", "1")?;
	app.register(entry)?;
	Ok(app)
}

fn main() -> ExitCode {
	match run() {
		Ok(code) => code,
		Err(error) => {
			eprintln!("{error}");
			ExitCode::FAILURE
		}
	}
}

fn run() -> Result<ExitCode, Box<dyn Error>> {
	let app = application()?;
	let args: Vec<_> = env::args_os().collect();

	if completion::requested() {
		let reply = completion::respond(app.cli_command(), args, &env::current_dir()?)?;
		io::stdout().write_all(&reply)?;
		return Ok(ExitCode::SUCCESS);
	}

	if let [_, command, shell] = args.as_slice()
		&& command == "completion"
	{
		let shell = shell.to_str().ok_or("Expected a shell name")?.parse()?;
		let script = completion::generate(&mut app.cli_command(), shell);
		if script.is_empty() {
			return Err("Choose bash, zsh, fish or powershell".into());
		}
		io::stdout().write_all(&script)?;
		return Ok(ExitCode::SUCCESS);
	}

	match app.parse_cli(args)? {
		Parsed::Display(help) => print!("{help}"),
		Parsed::Input(input) => {
			let report = input.run(CliViewContext::default(), Cancellation::new())?;
			return Ok(ExitCode::from(report.exit_code()));
		}
	}

	Ok(ExitCode::SUCCESS)
}
