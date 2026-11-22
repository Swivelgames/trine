//! On-demand RPC projection of the shared simulation; no Git effects.

#[path = "support/shared_workyard/mod.rs"]
mod workyard;

use std::process::ExitCode;
use trine::{
	Cancellation,
	rpc::{self, Config},
};

fn main() -> ExitCode {
	let result = run();
	match result {
		Ok(report) if report.failure.is_none() => ExitCode::SUCCESS,
		Ok(report) => {
			eprintln!("{}", report.failure.unwrap());
			ExitCode::FAILURE
		}
		Err(error) => {
			eprintln!("{error}");
			ExitCode::FAILURE
		}
	}
}

fn run() -> Result<rpc::HostReport, Box<dyn std::error::Error>> {
	let application = workyard::application()?;
	let shutdown = Cancellation::new();
	let signal = shutdown.clone();
	ctrlc::set_handler(move || signal.cancel())?;
	let (reader, writer) = rpc::stdio::open()?;
	Ok(smol::block_on(rpc::serve(
		&application,
		reader,
		writer,
		Config::default(),
		shutdown,
		|_| {},
	))?)
}
