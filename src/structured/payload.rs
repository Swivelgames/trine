//! Presentation values shared by structured views and their adapters.

use crate::error::FailureKind;
use serde_json::Value;

/// Terminal representation produced by a structured view.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
	/// Serialized successful domain outcome.
	Success(Value),
	/// Serialized domain error.
	DomainFailure(Value),
	/// Explicit application error presentation for the live RPC adapter.
	RpcError {
		/// Application-defined JSON-RPC error code outside the reserved protocol range.
		code: i64,
		/// Human-readable RPC error summary.
		message: String,
		/// Structured diagnostic data for the RPC caller.
		data: Value,
	},
	/// Failure reported by the framework when producing a terminal result.
	FrameworkFailure {
		/// Machine-readable framework classification.
		kind: FailureKind,
		/// Diagnostic for this invocation.
		message: String,
	},
}

/// Output retained by an in-process structured invocation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
	/// Framework diagnostics retained alongside the invocation's output.
	pub diagnostics: Vec<String>,
	/// Observed progress values in delivery order.
	pub progress: Vec<Value>,
	/// Terminal value, if one was accepted.
	pub terminal: Option<Payload>,
}
