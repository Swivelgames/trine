//! Invocation-scoped CLI output facilities and completion policy.

use std::{
	io::{self, Write},
	sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct Output(Arc<Mutex<Box<dyn Write + Send>>>);

impl Output {
	pub fn new(writer: impl Write + Send + 'static) -> Self {
		Self(Arc::new(Mutex::new(Box::new(writer))))
	}

	/// Writes and eagerly flushes under one lock, surfacing buffered output failures.
	pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
		let mut writer = self
			.0
			.lock()
			.map_err(|_| io::Error::other("output lock poisoned"))?;
		writer.write_all(bytes)?;
		writer.flush()
	}
}

#[derive(Clone)]
pub struct CliViewContext {
	pub stdout: Output,
	pub stderr: Output,
}

impl Default for CliViewContext {
	fn default() -> Self {
		Self {
			stdout: Output::new(io::stdout()),
			stderr: Output::new(io::stderr()),
		}
	}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CliCompletion {
	pub exit_code: Option<u8>,
}

impl CliCompletion {
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
