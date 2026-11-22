//! Direct CLI JSON envelopes, independent of RPC transport framing.
mod capture_view;
mod policy_view;
mod projection;

pub(crate) use projection::JsonProjection;

use super::Output;
use crate::{
	error::{Failure, FailureKind},
	execution::{ExecutionOutcome, ExecutionReport, PresentationFailure, PresentationState},
};
use serde::Serialize;
use serde_json::Value;

/// Machine-readable framework diagnostic in a CLI JSON response.
#[derive(Debug, Serialize)]
pub struct JsonError {
	/// Stable classification suitable for programmatic handling.
	pub code: &'static str,
	/// Human-readable detail about the failure.
	pub message: String,
}

/// Terminal document written for a CLI invocation using `--json`.
#[derive(Debug, Serialize)]
pub struct JsonResponse {
	/// Process exit code associated with this document.
	pub exit_code: u8,
	/// Serialized action outcome, when captured. This may contain a domain error.
	pub result: Option<Value>,
	/// Framework diagnostic for parsing, execution or presentation failure.
	pub error: Option<JsonError>,
}

impl JsonResponse {
	/// Construct a framework-error response with exit code one and no domain result.
	pub fn failure(kind: FailureKind, message: impl Into<String>) -> Self {
		Self {
			exit_code: 1,
			result: None,
			error: Some(JsonError {
				code: kind.as_str(),
				message: message.into(),
			}),
		}
	}

	pub(crate) fn from_execution<O, E>(
		report: &ExecutionReport<O, E, Failure, super::CliCompletion>,
		result: Option<Value>,
	) -> Self {
		let error = match (&report.outcome, &report.presentation) {
			(ExecutionOutcome::Failed(failure), _) => Some(JsonError {
				code: FailureKind::Panicked.as_str(),
				message: failure.message.clone(),
			}),
			(_, PresentationState::Failed(PresentationFailure::Output(failure))) => {
				Some(JsonError {
					code: failure.kind.as_str(),
					message: failure.to_string(),
				})
			}
			(_, PresentationState::Failed(PresentationFailure::Panicked)) => Some(JsonError {
				code: FailureKind::Panicked.as_str(),
				message: "JSON presentation panicked".into(),
			}),
			_ => None,
		};
		let exit_code = match (&error, &report.presentation) {
			(None, PresentationState::Completed(completion)) => completion
				.exit_code
				.unwrap_or(u8::from(!report.outcome.is_ok())),
			_ => 1,
		};
		Self {
			exit_code,
			result,
			error,
		}
	}

	/// Buffer before writing: an encoding failure must not publish half a document.
	pub fn write(&self, output: &Output) -> Result<(), Failure> {
		let mut bytes = Vec::new();
		let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
		let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
		self.serialize(&mut serializer)
			.map_err(|error| Failure::new(FailureKind::EncodingFailed, error))?;
		bytes.push(b'\n');
		output
			.write_all(&bytes)
			.map_err(|error| Failure::new(FailureKind::OutputFailed, error))
	}
}
