#[path = "mcp/admission.rs"]
mod admission;
#[path = "mcp/delivery.rs"]
mod delivery;
#[path = "mcp/failures.rs"]
mod failures;
#[path = "mcp/fixture.rs"]
mod fixture;
#[path = "mcp/progress.rs"]
mod progress;
#[path = "mcp/retained_progress.rs"]
mod retained_progress;

use fixture::*;
use serde_json::{Value, json};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, structured::StructuredBinding,
};

#[test]
fn negotiates_before_discovery_and_lists_only_bound_tools() {
	let requests = [
		request(0, "tools/list", json!({})),
		initialize(),
		request(2, "tools/list", json!({})),
		notification("notifications/initialized", json!({})),
		request(3, "ping", json!({})),
		request(4, "tools/list", json!({})),
		call(5, "hidden.echo"),
		call(6, "echo"),
	];
	let result = exchange(&catalog(), &requests);
	assert_eq!(result.len(), 7);
	assert_eq!(result[0]["error"]["code"], -32002);
	assert_eq!(
		result[1]["result"],
		json!({
			"protocolVersion": "2025-11-25", "capabilities": { "tools": {} },
			"serverInfo": { "name": "example", "version": "2.3" }
		})
	);
	assert_eq!(result[2]["error"]["code"], -32002);
	assert_eq!(result[3]["result"], json!({}));

	let tools = result[4]["result"]["tools"].as_array().unwrap();
	assert_eq!(tools.len(), 1);
	assert_eq!(tools[0]["name"], "echo");
	assert_eq!(tools[0]["description"], "Echo the input");
	assert_eq!(tools[0]["inputSchema"]["type"], "object");
	assert_eq!(result[5]["error"]["code"], -32602);

	let reply = &result[6]["result"];
	assert_eq!(reply["structuredContent"], json!({"value": null}));
	let text: Value = serde_json::from_str(reply["content"][0]["text"].as_str().unwrap()).unwrap();
	assert_eq!(text, reply["structuredContent"]);
}

#[test]
fn invalid_qualified_name_or_input_schema_stops_the_host_before_initialization() {
	let action = Action::new(
		"echo",
		"Echo the input",
		|_: Input, _: InvocationContext<()>| async { Ok::<(), ()>(()) },
	)
	.unwrap();
	// The path is 129 bytes once the namespace separator and action name are included.
	let long_name = Entry::new(action.clone())
		.namespace(["a".repeat(124)])
		.mcp(StructuredBinding::serde());
	let non_object = Entry::new(action)
		.mcp_with_input(
			StructuredBinding::serde(),
			"Echo a string",
			|value: String| Input { value: Some(value) },
		)
		.unwrap();

	for entry in [long_name, non_object] {
		let mut app = Application::new("invalid", "1").unwrap();
		app.register(entry).unwrap();

		let mut output = Vec::new();
		let error = serve(
			&app,
			lines(&[initialize()]),
			&mut output,
			Cancellation::new(),
		)
		.unwrap_err();
		assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
		assert!(output.is_empty());
	}
}

#[test]
fn distinguishes_protocol_errors_from_invalid_tool_inputs_and_accepts_the_next_call() {
	let mut requests = ready();
	requests.extend([
		request(2, "unknown", json!({})),
		request(3, "tools/call", json!({"name":"echo","arguments":[]})),
		request(
			4,
			"tools/call",
			json!({"name":"echo","arguments":{"typo":1}}),
		),
		notification("tools/call", json!({"name":"echo"})),
		call(5, "echo"),
	]);
	let mut bytes = b"malformed\n".to_vec();
	bytes.extend(lines(&requests));
	let mut output = Vec::new();
	serve(&catalog(), bytes, &mut output, Cancellation::new()).unwrap();
	let replies = replies(&output);

	assert_eq!(replies.len(), 6);
	assert_eq!(replies[0]["error"]["code"], -32700);
	assert!(replies[0].get("id").is_none());
	assert_eq!(replies[2]["error"]["code"], -32601);
	assert_eq!(replies[3]["error"]["code"], -32602);
	assert_eq!(replies[4]["result"]["isError"], true);
	assert!(
		replies[4]["result"]["content"][0]["text"]
			.as_str()
			.unwrap()
			.contains("typo")
	);
	assert!(replies[5]["result"]["structuredContent"].is_object());
}

#[test]
fn ping_and_client_cancellation_work_while_a_tool_owns_a_slot() {
	let settled = Cancellation::new();
	let mut app = waiting(Cancellation::new(), settled.clone());
	let observed = Action::new(
		"observe",
		"Observe settlement",
		move |_: Input, _: InvocationContext<()>| {
			let settled = settled.clone();
			async move {
				settled.cancelled().await;
				Ok::<_, ()>(json!({}))
			}
		},
	)
	.unwrap();
	app.register(Entry::new(observed).mcp(StructuredBinding::serde()))
		.unwrap();

	let long_id = "x".repeat(8192);
	let mut slow = call(2, "wait");
	slow["id"] = json!(long_id);
	let mut requests = ready();
	requests.extend([
		slow,
		request(3, "ping", json!({})),
		notification("notifications/cancelled", json!({"requestId":long_id})),
		notification("notifications/cancelled", json!({"requestId":999})),
		call(4, "observe"),
	]);
	let replies = exchange(&app, &requests);
	let ids: Vec<_> = replies
		.iter()
		.map(|reply| reply["id"].as_u64().unwrap())
		.collect();
	assert_eq!(ids, [1, 3, 4]);
}

#[test]
fn eof_and_host_shutdown_both_cancel_and_join_active_work() {
	for manual_shutdown in [false, true] {
		let shutdown = Cancellation::new();
		let started = if manual_shutdown {
			shutdown.clone()
		} else {
			Cancellation::new()
		};
		let settled = Cancellation::new();
		let app = waiting(started.clone(), settled.clone());
		let mut requests = ready();
		requests.push(call(2, "wait"));

		serve(&app, lines(&requests), Vec::new(), shutdown).unwrap();
		assert!(started.is_cancelled() && settled.is_cancelled());
	}
}

#[test]
fn oversized_input_is_rejected_and_the_host_reads_the_next_frame() {
	let mut bytes = vec![b'x'; (1 << 20) + 1];
	bytes.push(b'\n');
	bytes.extend(lines(&[request(1, "ping", json!({}))]));

	let mut output = Vec::new();
	serve(&catalog(), bytes, &mut output, Cancellation::new()).unwrap();
	let replies = replies(&output);
	assert_eq!(replies.len(), 2);
	assert_eq!(replies[0]["error"]["code"], -32001);
	assert_eq!(replies[1]["result"], json!({}));
}

#[test]
fn dispatches_a_discovered_namespaced_tool_and_rejects_unimplemented_call_options() {
	let mut app = catalog();
	let invocations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
	let observed = invocations.clone();
	let name = Action::new(
		"identity",
		"Return the project",
		move |_: Input, _: InvocationContext<()>| {
			observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
			async { Ok::<_, ()>(json!({ "project": "example" })) }
		},
	)
	.unwrap();
	app.register(
		Entry::new(name)
			.namespace(["project"])
			.mcp(StructuredBinding::serde()),
	)
	.unwrap();

	let mut requests = ready();
	requests.extend([
		request(2, "tools/list", json!({})),
		request(
			3,
			"tools/call",
			json!({"name": "project.identity", "task": {}}),
		),
		call(4, &"a".repeat(129)),
		call(5, "project.identity"),
	]);
	let result = exchange(&app, &requests);
	assert!(
		result[1]["result"]["tools"]
			.as_array()
			.unwrap()
			.iter()
			.any(|tool| tool["name"] == "project.identity")
	);
	assert_eq!(result[2]["id"], 3);
	assert_eq!(result[2]["error"]["code"], -32602);
	assert_eq!(result[3]["error"]["code"], -32602);
	assert_eq!(
		result[4]["result"]["structuredContent"],
		json!({"project":"example"})
	);
	assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn reusing_an_active_id_cannot_take_the_response_from_the_original_caller() {
	let mut app = catalog();
	let release = Cancellation::new();
	let gate = release.clone();
	let original = Action::new(
		"original",
		"Wait for release",
		move |_: Input, _: InvocationContext<()>| {
			let gate = gate.clone();
			async move {
				gate.cancelled().await;
				Ok::<_, ()>(json!("original"))
			}
		},
	)
	.unwrap();
	let release = Action::new(
		"release",
		"Release original",
		move |_: Input, _: InvocationContext<()>| {
			let release = release.clone();
			async move {
				release.cancel();
				Ok::<_, ()>(json!("released"))
			}
		},
	)
	.unwrap();
	app.register(Entry::new(original).mcp(StructuredBinding::serde()))
		.unwrap();
	app.register(Entry::new(release).mcp(StructuredBinding::serde()))
		.unwrap();

	let mut requests = ready();
	requests.extend([
		call(2, "original"),
		call(2, "echo"),
		json!({"jsonrpc": "wrong", "id": 2}),
		json!({"jsonrpc": "wrong", "id": 3}),
		call(4, "release"),
		call(5, "echo"),
	]);
	let result = exchange(&app, &requests);
	for rejection in &result[1..3] {
		assert_eq!(rejection["error"]["code"], -32600);
		assert!(rejection.get("id").is_none());
	}
	assert_eq!(result[3]["id"], 3);
	assert_eq!(result[3]["error"]["code"], -32600);

	let matching: Vec<_> = result.iter().filter(|reply| reply["id"] == 2).collect();
	assert_eq!(matching.len(), 1);
	assert_eq!(matching[0]["result"]["content"][0]["text"], "\"original\"");
	assert!(result.last().unwrap()["result"]["structuredContent"].is_object());
}

#[test]
fn combined_text_and_structured_content_can_exceed_the_output_frame_budget() {
	let mut app = catalog();
	let large = Action::new(
		"large",
		"Large result",
		|_: Input, _: InvocationContext<()>| async {
			Ok::<_, ()>(json!({ "value": "x".repeat(600_000) }))
		},
	)
	.unwrap();
	app.register(Entry::new(large).mcp(StructuredBinding::serde()))
		.unwrap();
	let mut requests = ready();
	requests.extend([call(2, "large"), request(3, "ping", json!({}))]);

	let result = exchange(&app, &requests);
	assert_eq!(result[1]["id"], 2);
	assert_eq!(result[1]["error"]["code"], -32603);
	assert!(result[1].to_string().len() < 256);
	assert_eq!(result[2]["result"], json!({}));
}

#[test]
fn error_correlation_survives_an_id_that_fills_the_input_frame() {
	let mut unknown = request(2, "x", json!({}));
	unknown["id"] = json!("");
	let frame_limit = 1 << 20;
	let id = "x".repeat(frame_limit - unknown.to_string().len());
	unknown["id"] = json!(id);
	assert_eq!(unknown.to_string().len(), frame_limit);

	let result = exchange(&catalog(), &[unknown, request(3, "ping", json!({}))]);
	assert_eq!(result[0]["id"], id);
	assert!(result[0].get("error").is_some());
	assert!(result[0].to_string().len() <= frame_limit + 128);
	assert_eq!(result[1]["result"], json!({}));
}
