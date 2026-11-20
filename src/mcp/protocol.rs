//! Decode the JSON-RPC envelope before interpreting MCP methods.

use super::FRAME_BYTES;
use crate::jsonrpc::Id;
use serde_json::{Value, json};

pub(super) struct Request {
	pub id: Option<Id>,
	pub method: String,
	pub params: Value,
}

pub(super) fn decode(bytes: &[u8], id_in_use: impl FnOnce(&Id) -> bool) -> Result<Request, Value> {
	let mut value: Value =
		serde_json::from_slice(bytes).map_err(|_| error(Value::Null, -32700, "Invalid JSON"))?;
	let id = Id::parse(&value["id"], FRAME_BYTES);
	// A malformed envelope can carry the ID of a still-running call too.
	if id.as_ref().is_some_and(id_in_use) {
		return Err(error(Value::Null, -32600, "Request ID is already in use"));
	}
	let invalid = || {
		error(
			id.as_ref().map_or(Value::Null, Id::value),
			-32600,
			"Invalid JSON-RPC request",
		)
	};
	let object = value.as_object_mut().ok_or_else(invalid)?;
	if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
		|| object.contains_key("result")
		|| object.contains_key("error")
		|| (object.contains_key("id") && id.is_none())
	{
		return Err(invalid());
	}
	let method = object
		.get("method")
		.and_then(Value::as_str)
		.ok_or_else(invalid)?
		.to_owned();
	let params = object.remove("params").unwrap_or_else(|| json!({}));
	Ok(Request { id, method, params })
}

pub(super) fn success(id: &Id, result: Value) -> Value {
	json!({ "jsonrpc": "2.0", "id": id.value(), "result": result })
}

pub(super) fn error(id: Value, code: i32, message: &str) -> Value {
	let mut response = json!({ "jsonrpc": "2.0", "error": { "code": code, "message": message } });
	if !id.is_null() {
		response["id"] = id;
	}
	response
}
