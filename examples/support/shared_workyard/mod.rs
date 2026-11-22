//! Shared catalog for the CLI, RPC and in-process examples.

mod create;
mod list;

use trine::Application;

pub fn application() -> Result<Application, Box<dyn std::error::Error>> {
	let mut app = Application::new("workyard-demo", env!("CARGO_PKG_VERSION"))?;
	create::register(&mut app)?;
	list::register(&mut app)?;
	Ok(app)
}

#[cfg(test)]
mod tests {
	use super::application;
	use std::io;
	use trine::{
		Cancellation,
		application::{CliViewContext, Output},
		cli::Parsed,
	};

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
						stdout: Output::new(io::sink()),
						stderr: Output::new(io::sink()),
					},
					Cancellation::new(),
				)
				.unwrap();
			assert_eq!(report.exit_code(), 0);
			assert!(!report.diagnostics.attempted);
		}
	}
}
