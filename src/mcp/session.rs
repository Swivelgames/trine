//! Negotiation, capability discovery and method admission for one client.

use super::{
	FRAME_BYTES, REVISION,
	protocol::{self, Request},
};
use crate::{Application, application::Interface, jsonrpc::Id};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io;

const MAX_TOOL_NAME_BYTES: usize = 128;

#[derive(PartialEq)]
enum Phase {
	New,
	Initializing,
	Ready,
}

pub(super) enum Dispatch {
	Ignore,
	Reply(Value),
	Cancel(Id),
	Call {
		id: Id,
		name: String,
		arguments: Value,
		token: Option<Id>,
	},
}

#[derive(Deserialize)]
struct CallParams {
	name: String,
	#[serde(default)]
	arguments: serde_json::Map<String, Value>,
}

pub(super) struct Session<'a> {
	application: &'a Application,
	phase: Phase,
}

impl<'a> Session<'a> {
	pub fn new(application: &'a Application) -> io::Result<Self> {
		for tool in application.discover(Interface::Mcp) {
			if tool.input_schema["type"] != "object"
				|| tool.path.join(".").len() > MAX_TOOL_NAME_BYTES
			{
				return Err(io::Error::new(
					io::ErrorKind::InvalidInput,
					format!(
						"MCP tool {} requires an object input schema and a name of at most 128 bytes",
						tool.path.join(".")
					),
				));
			}
		}
		Ok(Self {
			application,
			phase: Phase::New,
		})
	}

	pub fn route(&mut self, Request { id, method, params }: Request) -> Dispatch {
		let Some(id) = id else {
			return self.notification(&method, &params);
		};
		if !params.is_object() {
			return Self::invalid(&id, "Request params must be an object");
		}

		match method.as_str() {
			"ping" => Dispatch::Reply(protocol::success(&id, json!({}))),
			"initialize" => self.initialize(&id, &params),
			"tools/list" | "tools/call" if self.phase != Phase::Ready => Dispatch::Reply(
				protocol::error(id.value(), -32002, "Server has not finished initialization"),
			),
			"tools/list" => self.list(&id, &params),
			"tools/call" => Self::call(id, params),
			_ => Dispatch::Reply(protocol::error(id.value(), -32601, "Unknown method")),
		}
	}

	fn call(id: Id, params: Value) -> Dispatch {
		if params.get("task").is_some() {
			return Self::invalid(&id, "Task-augmented tool calls are unsupported");
		}
		let token = match progress_token(&params) {
			Ok(token) => token,
			Err(message) => return Self::invalid(&id, message),
		};

		let Ok(call) = serde_json::from_value::<CallParams>(params) else {
			return Self::invalid(&id, "Expected a tool name and optional object arguments");
		};
		if call.name.len() > MAX_TOOL_NAME_BYTES {
			return Self::invalid(&id, "Tool name exceeds 128 bytes");
		}

		Dispatch::Call {
			id,
			name: call.name,
			arguments: call.arguments.into(),
			token,
		}
	}

	fn notification(&mut self, method: &str, params: &Value) -> Dispatch {
		if !params.is_object() {
			return Dispatch::Ignore;
		}
		match method {
			"notifications/initialized" if self.phase == Phase::Initializing => {
				self.phase = Phase::Ready
			}
			"notifications/cancelled" if self.phase == Phase::Ready => {
				if let Some(id) = Id::parse(&params["requestId"], FRAME_BYTES) {
					return Dispatch::Cancel(id);
				}
			}
			_ => {}
		}
		Dispatch::Ignore
	}

	fn initialize(&mut self, id: &Id, params: &Value) -> Dispatch {
		if self.phase != Phase::New {
			return Self::invalid(id, "Server is already initialized");
		}
		if !params["protocolVersion"].is_string()
			|| !params["capabilities"].is_object()
			|| !params["clientInfo"]["name"].is_string()
			|| !params["clientInfo"]["version"].is_string()
		{
			return Self::invalid(
				id,
				"Expected protocolVersion, capabilities and clientInfo with name and version",
			);
		}

		self.phase = Phase::Initializing;
		let (name, version) = self.application.identity();
		let result = json!({
			"protocolVersion": REVISION,
			"capabilities": { "tools": {} },
			"serverInfo": { "name": name, "version": version }
		});
		Dispatch::Reply(protocol::success(id, result))
	}

	fn list(&self, id: &Id, params: &Value) -> Dispatch {
		if params.get("cursor").is_some() {
			return Self::invalid(id, "Unknown tool list cursor");
		}
		let tools: Vec<_> = self
			.application
			.discover(Interface::Mcp)
			.into_iter()
			.map(|tool| {
				json!({
					"name": tool.path.join("."),
					"description": tool.description,
					"inputSchema": tool.input_schema,
				})
			})
			.collect();
		Dispatch::Reply(protocol::success(id, json!({ "tools": tools })))
	}

	fn invalid(id: &Id, message: &str) -> Dispatch {
		Dispatch::Reply(protocol::error(id.value(), -32602, message))
	}
}

fn progress_token(params: &Value) -> Result<Option<Id>, &'static str> {
	let Some(meta) = params.get("_meta") else {
		return Ok(None);
	};
	let meta = meta.as_object().ok_or("Expected object _meta")?;
	meta.get("progressToken")
		.map(|value| {
			Id::parse(value, FRAME_BYTES).ok_or("Expected a string or integer progressToken")
		})
		.transpose()
}
