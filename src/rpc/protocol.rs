//! JSON-RPC envelope validation and request decoding.

use serde_json::{Value, json};

pub(crate) use crate::jsonrpc::Id;

pub(crate) enum Request {
	Invoke {
		id: Id,
		path: Vec<String>,
		input: Value,
		observe: bool,
	},
	Cancel(Id),
	Reject(Value),
	Ignore,
}

pub(crate) fn error(id: Value, code: i64, kind: &str, message: &str) -> Value {
	json!({
		"jsonrpc": "2.0",
		"id": id,
		"error": {
			"code": code,
			"message": message,
			"data": { "code": kind },
		},
	})
}

pub(crate) fn decode(bytes: &[u8], id_limit: usize) -> Request {
	let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
		return Request::Reject(error(Value::Null, -32700, "parse_error", "Invalid JSON"));
	};

	let invalid = || {
		Request::Reject(error(
			Value::Null,
			-32600,
			"invalid_request",
			"Invalid request",
		))
	};

	if value.is_array() {
		return Request::Reject(error(
			Value::Null,
			-32600,
			"batch_unsupported",
			"Batch requests are unsupported",
		));
	}
	let Some(object) = value.as_object() else {
		return invalid();
	};
	if object.get("jsonrpc") != Some(&json!("2.0"))
		|| !object.get("method").is_some_and(Value::is_string)
	{
		return invalid();
	}

	let id = match object.get("id") {
		Some(value) => match Id::parse(value, id_limit) {
			Some(id) => Some(id),
			None => {
				let kind = if value.as_str().is_some_and(|id| id.len() > id_limit) {
					"id_too_large"
				} else {
					"invalid_id"
				};
				return Request::Reject(error(Value::Null, -32600, kind, "Invalid request ID"));
			}
		},
		None => None,
	};

	let method = object["method"].as_str().unwrap();
	let params = object.get("params");
	if method == "cancel" {
		if let Some(id) = id {
			return Request::Reject(error(
				id.value(),
				-32600,
				"invalid_request",
				"Cancel must be a notification",
			));
		}
		return params
			.and_then(|p| p.get("id"))
			.and_then(|id| Id::parse(id, id_limit))
			.map_or(Request::Ignore, Request::Cancel);
	}

	let Some(id) = id else {
		return Request::Ignore;
	};
	if method != "invoke" {
		return Request::Reject(error(
			id.value(),
			-32601,
			"method_not_found",
			"Unknown method",
		));
	}

	let parsed = params.and_then(|p| {
		let path = p
			.get("action")?
			.as_array()?
			.iter()
			.map(|v| v.as_str().map(str::to_owned))
			.collect::<Option<Vec<_>>>()?;
		if path.is_empty() || path.iter().any(String::is_empty) {
			return None;
		}

		let input = p.get("input")?.clone();
		let observe = match p.get("observe") {
			None => false,
			Some(v) => v.as_bool()?,
		};
		Some((path, input, observe))
	});

	match parsed {
		Some((path, input, observe)) => Request::Invoke {
			id,
			path,
			input,
			observe,
		},
		None => Request::Reject(error(
			id.value(),
			-32602,
			"invalid_params",
			"Invalid invocation parameters",
		)),
	}
}
