//! CLI presentation for the Workyard simulation: progress on stderr, results on stdout.

mod support {
	pub mod workyard;
}

use std::{num::NonZeroUsize, process::ExitCode, time::Duration};

use support::workyard;
use trine::{Cancellation, cli::Parsed};

fn main() -> ExitCode {
	// Handle help and parse errors before starting the runtime or signal handling.
	let cli = workyard::cli(workyard::action(Duration::from_millis(250)));
	let input = match cli.parse(std::env::args_os()) {
		Ok(Parsed::Display(text)) => {
			print!("{text}");
			return ExitCode::SUCCESS;
		}
		Ok(Parsed::Input(input)) => input,
		Err(error) => {
			eprintln!("{error}");
			return ExitCode::FAILURE;
		}
	};

	// Ctrl+C requests cancellation; the handler decides how to finish safely.
	let cancellation = Cancellation::new();
	let signal_cancellation = cancellation.clone();
	if let Err(error) = ctrlc::set_handler(move || signal_cancellation.cancel()) {
		eprintln!("Cannot install Ctrl+C handling: {error}. Restart this simulation to retry.");
		return ExitCode::FAILURE;
	}

	eprintln!("Simulation only: no repositories or workspace state will be changed.");
	let result = cli.run(
		input,
		NonZeroUsize::new(16).unwrap(),
		cancellation,
		|event| {
			eprintln!("{}: {}", event.repository, event.phase);
		},
	);

	// The runner drains accepted progress before returning the terminal result.
	match result {
		Ok(outcome) => {
			let response = serde_json::json!({ "status": "completed", "outcome": outcome });
			println!("{}", serde_json::to_string_pretty(&response).unwrap());
			ExitCode::SUCCESS
		}
		Err(error) => {
			println!("{}", serde_json::to_string_pretty(&error).unwrap());
			ExitCode::FAILURE
		}
	}
}
