//! Select the CLI, RPC or MCP host for a tutorial application.

use std::{error::Error, process::ExitCode};
use trine::{Application, Cancellation, application::CliViewContext, cli::Parsed};

pub fn run(app: Application) -> Result<ExitCode, Box<dyn Error>> {
	let cancellation = Cancellation::new();
	let mut args = std::env::args_os().skip(1);

	match (
		args.next().as_deref().and_then(|arg| arg.to_str()),
		args.next(),
	) {
		(Some("rpc"), None) => trine::rpc::run_stdio(&app, cancellation)?,
		(Some("mcp"), None) => trine::mcp::run_stdio(&app, cancellation)?,
		_ => return run_cli(&app, cancellation),
	}

	Ok(ExitCode::SUCCESS)
}

fn run_cli(app: &Application, cancellation: Cancellation) -> Result<ExitCode, Box<dyn Error>> {
	match app.parse_cli(std::env::args_os())? {
		Parsed::Display(text) => {
			print!("{text}");
			Ok(ExitCode::SUCCESS)
		}
		Parsed::Input(input) => {
			let report = input.run(CliViewContext::default(), cancellation)?;
			Ok(ExitCode::from(report.exit_code()))
		}
	}
}
