//! Framework classifications retain diagnostic causes independently of domain errors.

use std::{error::Error, fmt, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
	Unavailable,
	InvalidInput,
	ViewSetupFailed,
	EncodingFailed,
	OutputTooLarge,
	OutputFailed,
	ContractViolation,
	Panicked,
}

#[derive(Debug)]
struct Message(String);

impl fmt::Display for Message {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl Error for Message {}

#[derive(Clone, Debug)]
pub struct Failure {
	pub kind: FailureKind,
	cause: Arc<dyn Error + Send + Sync>,
}

impl Failure {
	pub fn new(kind: FailureKind, cause: impl Error + Send + Sync + 'static) -> Self {
		Self {
			kind,
			cause: Arc::new(cause),
		}
	}

	pub fn message(kind: FailureKind, message: impl Into<String>) -> Self {
		Self::new(kind, Message(message.into()))
	}

	pub fn from_box(kind: FailureKind, cause: Box<dyn Error + Send + Sync>) -> Self {
		match cause.downcast::<Self>() {
			Ok(failure) => *failure,
			Err(cause) => Self {
				kind,
				cause: Arc::from(cause),
			},
		}
	}
}

impl fmt::Display for Failure {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:?}: {}", self.kind, self.cause)
	}
}

impl Error for Failure {
	fn source(&self) -> Option<&(dyn Error + 'static)> {
		Some(self.cause.as_ref())
	}
}

impl From<String> for Failure {
	fn from(message: String) -> Self {
		Self::message(FailureKind::OutputFailed, message)
	}
}

impl From<&str> for Failure {
	fn from(message: &str) -> Self {
		Self::from(message.to_owned())
	}
}
