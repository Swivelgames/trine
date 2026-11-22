//! Framework classifications retain diagnostic causes independently of domain errors.

use std::{error::Error, fmt, sync::Arc};

/// Classification of a framework failure for routing, reporting and JSON error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
	/// The requested action, host resource or interface is unavailable.
	Unavailable,
	/// Input could not be parsed or validated.
	InvalidInput,
	/// The view could not be constructed for this invocation.
	ViewSetupFailed,
	/// A presentation value could not be encoded.
	EncodingFailed,
	/// The encoded output exceeds the allowed size.
	OutputTooLarge,
	/// Writing or delivering view output failed.
	OutputFailed,
	/// An adapter or view violated the invocation contract.
	ContractViolation,
	/// Execution or presentation unwound with a panic.
	Panicked,
}

impl FailureKind {
	/// Return the snake-case error code used in structured diagnostics.
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Unavailable => "unavailable",
			Self::InvalidInput => "invalid_input",
			Self::ViewSetupFailed => "view_setup_failed",
			Self::EncodingFailed => "encoding_failed",
			Self::OutputTooLarge => "output_too_large",
			Self::OutputFailed => "output_failed",
			Self::ContractViolation => "contract_violation",
			Self::Panicked => "panicked",
		}
	}
}

#[derive(Debug)]
struct Message(String);

impl fmt::Display for Message {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl Error for Message {}

/// A classified framework error retaining its cause for diagnostics.
#[derive(Clone, Debug)]
pub struct Failure {
	/// Machine-readable category of this failure.
	pub kind: FailureKind,
	cause: Arc<dyn Error + Send + Sync>,
}

impl Failure {
	/// Wrap a concrete error while preserving its source.
	pub fn new(kind: FailureKind, cause: impl Error + Send + Sync + 'static) -> Self {
		Self {
			kind,
			cause: Arc::new(cause),
		}
	}

	/// Construct a classified failure from diagnostic text.
	pub fn message(kind: FailureKind, message: impl Into<String>) -> Self {
		Self::new(kind, Message(message.into()))
	}

	/// Classify a boxed error, preserving its original classification if it is already a Trine
	/// failure.
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
