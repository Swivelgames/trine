//! Invocation-scoped CLI output facilities and completion policy.

#[cfg(feature = "cli-confirmation")]
mod confirmation;
#[cfg(feature = "cli-confirmation")]
mod prompt;
#[cfg(feature = "cli-confirmation")]
mod selection;

use std::{
	io::{self, IsTerminal, Write},
	sync::{Arc, Mutex},
};

/// A clonable, synchronized output writer with an explicit terminal capability.
#[derive(Clone)]
pub struct Output {
	writer: Arc<Mutex<Box<dyn Write + Send>>>,
	terminal: bool,
}

impl Output {
	/// Wrap a writer as a plain output stream. Use `with_terminal` for interactive output.
	pub fn new(writer: impl Write + Send + 'static) -> Self {
		Self::with_terminal(writer, false)
	}

	/// The host supplies terminal capability; arbitrary writers default to plain output.
	pub fn with_terminal(writer: impl Write + Send + 'static, terminal: bool) -> Self {
		Self {
			writer: Arc::new(Mutex::new(Box::new(writer))),
			terminal,
		}
	}

	/// Return the terminal capability supplied by the host.
	pub fn is_terminal(&self) -> bool {
		self.terminal
	}

	/// Writes and eagerly flushes under one lock, surfacing buffered output failures.
	pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
		let mut writer = self
			.writer
			.lock()
			.map_err(|_| io::Error::other("output lock poisoned"))?;
		writer.write_all(bytes)?;
		writer.flush()
	}
}

/// Output streams for one CLI view. The default uses process stdout and stderr.
#[derive(Clone)]
pub struct CliViewContext {
	/// Destination for the command's primary output.
	pub stdout: Output,
	/// Destination for progress and diagnostics.
	pub stderr: Output,
}

impl Default for CliViewContext {
	fn default() -> Self {
		Self {
			stdout: Output::with_terminal(io::stdout(), io::stdout().is_terminal()),
			stderr: Output::with_terminal(io::stderr(), io::stderr().is_terminal()),
		}
	}
}

/// The CLI view's completion metadata used to choose a process exit code.
#[derive(Clone, Copy, Debug, Default)]
pub struct CliCompletion {
	/// Explicit process exit code, or `None` to use the domain outcome.
	pub exit_code: Option<u8>,
}

impl CliCompletion {
	/// Set an explicit exit code for a completed presentation.
	pub fn exit(code: u8) -> Self {
		Self {
			exit_code: Some(code),
		}
	}
}

impl From<()> for CliCompletion {
	fn from(_: ()) -> Self {
		Self::default()
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn custom_outputs_are_plain_unless_host_marks_them_terminal() {
		let plain = super::Output::new(Vec::new());
		let terminal = super::Output::with_terminal(Vec::new(), true);
		assert!(!plain.is_terminal());
		assert!(terminal.clone().is_terminal());
		terminal.write_all(b"output").unwrap();
	}
}
