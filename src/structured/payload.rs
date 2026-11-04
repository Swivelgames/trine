//! Presentation values shared by structured views and their adapters.

use crate::error::FailureKind;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
	Success(Value),
	DomainFailure(Value),
	/// Explicit application error presentation for the live RPC adapter.
	RpcError {
		code: i64,
		message: String,
		data: Value,
	},
	FrameworkFailure {
		kind: FailureKind,
		message: String,
	},
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
	pub diagnostics: Vec<String>,
	pub progress: Vec<Value>,
	pub terminal: Option<Payload>,
}
