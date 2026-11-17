//! Shell scripts generated from the same Clap tree used for parsing.
use clap::Command;
pub use clap_complete::Shell;

/// Generate into memory so a failing output stream cannot panic inside a generator.
pub fn generate(command: &mut Command, shell: Shell) -> Vec<u8> {
	let mut script = Vec::new();
	let program = command.get_name().to_owned();
	clap_complete::generate(shell, command, program, &mut script);
	script
}
