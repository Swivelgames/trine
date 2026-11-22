//! CLI projection of the shared Workyard simulation catalog.

#[path = "support/shared_workyard/mod.rs"]
mod workyard;

use std::process::ExitCode;
use trine::{Cancellation, application::CliViewContext, cli::Parsed};

fn main() -> ExitCode {
	let result = workyard::application()
		.and_then(|app| app.parse_cli(std::env::args_os()).map_err(Into::into));

	let prepared = match result {
		Ok(Parsed::Display(text)) => {
			print!("{text}");
			return ExitCode::SUCCESS;
		}
		Ok(Parsed::Input(prepared)) => prepared,
		Err(error) => {
			eprintln!("{error}");
			return ExitCode::FAILURE;
		}
	};

	// Install signal handling only after help and input parsing have completed.
	let cancellation = Cancellation::new();
	let signal_cancellation = cancellation.clone();
	if let Err(error) = ctrlc::set_handler(move || signal_cancellation.cancel()) {
		eprintln!("{error}");
		return ExitCode::FAILURE;
	}

	let report = match prepared.run(CliViewContext::default(), cancellation) {
		Ok(report) => report,
		Err(error) => {
			eprintln!("{error}");
			return ExitCode::FAILURE;
		}
	};

	ExitCode::from(report.exit_code())
}
