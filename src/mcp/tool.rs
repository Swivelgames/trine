//! Translate a single typed invocation into an MCP tool result.

use super::{FRAME_BYTES, progress::Progress, protocol};
use crate::{
	Cancellation,
	application::{InvocationError, McpTool, PreparedStructured, StructuredReport},
	jsonrpc::{Id, framing},
	structured::Payload,
};
use serde_json::{Value, json};
use std::{
	panic::{AssertUnwindSafe, catch_unwind},
	sync::Arc,
};

pub(super) fn prepare(
	tool: McpTool<'_>,
	id: &Id,
	arguments: Value,
	cancel: Cancellation,
	progress: Option<Arc<Progress>>,
) -> Result<PreparedStructured, Value> {
	catch_unwind(AssertUnwindSafe(|| {
		tool.prepare(arguments, cancel, progress)
			.map_err(|error| preparation_error(id, error))
	}))
	.unwrap_or_else(|_| {
		Err(protocol::error(
			id.value(),
			-32603,
			"MCP tool preparation panicked",
		))
	})
}

fn preparation_error(id: &Id, error: InvocationError) -> Value {
	match error {
		InvocationError::Unavailable => protocol::error(id.value(), -32602, "Unknown tool"),
		InvocationError::Input(error) => {
			let content = json!([{ "type": "text", "text": error.to_string() }]);
			protocol::success(id, json!({ "isError": true, "content": content }))
		}
		InvocationError::Setup(error) => protocol::error(id.value(), -32603, &error.to_string()),
	}
}

pub(super) fn outcome(id: &Id, report: StructuredReport) -> Value {
	if report.execution.execution_failure.is_some() || !report.execution.presentation_succeeded {
		let fallback = report
			.execution
			.execution_failure
			.as_ref()
			.map_or("MCP tool presentation failed", |fault| {
				fault.message.as_str()
			});
		let message = match &report.transcript.terminal {
			Some(Payload::FrameworkFailure { message, .. }) => message,
			_ => report
				.transcript
				.diagnostics
				.last()
				.map_or(fallback, String::as_str),
		};
		return protocol::error(id.value(), -32603, message);
	}

	let value = match report.transcript.terminal {
		Some(Payload::Success(value)) if report.execution.domain_succeeded => value,
		Some(Payload::DomainFailure(value)) if !report.execution.domain_succeeded => value,
		Some(Payload::Success(_) | Payload::DomainFailure(_)) => {
			return protocol::error(
				id.value(),
				-32603,
				"MCP tool presentation contradicts the action outcome",
			);
		}
		Some(Payload::FrameworkFailure { message, .. }) => {
			return protocol::error(id.value(), -32603, &message);
		}
		_ => {
			return protocol::error(
				id.value(),
				-32603,
				"Missing or incompatible MCP tool result",
			);
		}
	};

	// Bound the duplicate JSON representation needed by text-only clients.
	let Ok(mut bytes) = framing::encode(&value, FRAME_BYTES) else {
		return protocol::error(
			id.value(),
			-32603,
			"MCP tool result exceeds the frame limit",
		);
	};
	bytes.pop(); // framing newline
	let text = String::from_utf8(bytes).expect("JSON encoder emits UTF-8");
	let mut result = json!({ "content": [{ "type": "text", "text": text }] });
	if value.is_object() {
		result["structuredContent"] = value;
	}
	if !report.execution.domain_succeeded {
		result["isError"] = true.into();
	}
	protocol::success(id, result)
}
