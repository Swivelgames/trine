//! Classified failures before an invocation can begin.

use std::fmt;

#[derive(Debug)]
pub enum InvocationError {
	Unavailable,
	Input(crate::error::Failure),
	Setup(crate::error::Failure),
}

impl fmt::Display for InvocationError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Unavailable => f.write_str("action unavailable for this interface"),
			Self::Input(e) => write!(f, "invalid input: {e}"),
			Self::Setup(e) => write!(f, "view setup failed: {e}"),
		}
	}
}

impl InvocationError {
	pub fn kind(&self) -> crate::error::FailureKind {
		use crate::error::FailureKind;
		match self {
			Self::Unavailable => FailureKind::Unavailable,
			Self::Input(_) => FailureKind::InvalidInput,
			Self::Setup(_) => FailureKind::ViewSetupFailed,
		}
	}
}

impl std::error::Error for InvocationError {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Unavailable => None,
			Self::Input(error) | Self::Setup(error) => Some(error),
		}
	}
}
