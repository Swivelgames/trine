use super::fixture::*;
use serde_json::{Value, json};
use std::{
	num::NonZeroUsize,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
};
use trine::{Action, Cancellation, Entry, InvocationContext, structured::StructuredBinding};

#[test]
fn interleaved_calls_keep_their_own_counts_and_terminal_results() {
	let second = Cancellation::new();
	let mut app = catalog();
	let action = Action::new(
		"work",
		"Report participant work",
		move |input: Input, context: InvocationContext<String>| {
			let second = second.clone();
			async move {
				let name = input.value.unwrap();
				context.emit(format!("Begin {name}")).await.unwrap();
				if name == "first" {
					second.cancelled().await;
				} else {
					second.cancel();
				}
				context.emit(format!("End {name}")).await.unwrap();
				let result = json!({"participant":name});
				if name == "second" {
					Err(result)
				} else {
					Ok(result)
				}
			}
		},
	)
	.unwrap();
	app.register(Entry::new(action).mcp(StructuredBinding::serde_progress(
		NonZeroUsize::new(4).unwrap(),
	)))
	.unwrap();

	let mut requests = ready();
	for (id, token, name) in [(2, json!("7"), "first"), (3, json!(7), "second")] {
		let mut call = subscribed(id, "work", token);
		call["params"]["arguments"] = json!({"value":name});
		requests.push(call);
	}
	let frames = exchange(&app, &requests);
	assert_eq!(frames.len(), 7);
	assert_eq!(frames[1]["params"]["message"], "Begin first");
	for (id, token, name) in [(2, json!("7"), "first"), (3, json!(7), "second")] {
		let frames: Vec<_> = frames
			.iter()
			.filter(|frame| frame["id"] == id || frame["params"]["progressToken"] == token)
			.collect();
		assert_eq!(frames.len(), 3);
		for (index, verb) in ["Begin", "End"].into_iter().enumerate() {
			assert_eq!(
				*frames[index],
				notification(
					"notifications/progress",
					json!({
						"progressToken":token, "progress":index + 1, "message":format!("{verb} {name}")
					})
				)
			);
		}
		let result = &frames[2]["result"];
		let value = json!({"participant":name});
		assert_eq!(result["structuredContent"], value);
		assert_eq!(result["content"][0]["text"], value.to_string());
		assert_eq!(result["isError"] == true, name == "second");
	}
}

#[test]
fn invalid_and_active_tokens_are_rejected_before_execution_and_can_be_reused_after_settlement() {
	let mut app = waiting(Cancellation::new(), Cancellation::new());
	let runs = Arc::new(AtomicUsize::new(0));
	let observed = runs.clone();
	let action = Action::new(
		"count",
		"Count invocations",
		move |_: Input, context: InvocationContext<u8>| {
			observed.fetch_add(1, Ordering::Relaxed);
			async move {
				context.emit(42).await.unwrap();
				Ok::<_, ()>(json!({}))
			}
		},
	)
	.unwrap();
	app.register(Entry::new(action).mcp(StructuredBinding::serde_progress(NonZeroUsize::MIN)))
		.unwrap();

	let mut requests = ready();
	requests.push(subscribed(2, "wait", json!(-7))); // Terminal-only tools still reserve supplied tokens.
	for (index, token) in [
		Value::Null,
		json!(false),
		json!([]),
		json!({}),
		json!(2.5),
		json!(-7),
	]
	.into_iter()
	.enumerate()
	{
		requests.push(subscribed(3 + index as u32, "count", token));
	}
	requests.extend([
		notification("notifications/cancelled", json!({"requestId":2})),
		call(9, "count"), // Tokenless invocation receives a terminal reply only.
		subscribed(10, "count", json!(-7)),
		subscribed(11, "count", json!(-7)),
	]);
	let frames = exchange(&app, &requests);
	assert_eq!(runs.load(Ordering::Relaxed), 3);
	assert_eq!(frames.len(), 12);
	for (index, rejected) in frames[1..7].iter().enumerate() {
		assert_eq!(rejected["id"], index + 3);
		assert_eq!(rejected["error"]["code"], -32602);
	}
	assert_eq!(frames[7]["id"], 9);
	for (pair, id) in frames[8..].chunks_exact(2).zip([10, 11]) {
		assert_eq!(
			pair[0],
			notification(
				"notifications/progress",
				json!({"progressToken":-7, "progress":1})
			)
		);
		assert_eq!(pair[1]["id"], id);
		assert_eq!(pair[1]["result"]["structuredContent"], json!({}));
	}
}

#[test]
fn oversized_progress_is_a_correlated_presentation_error() {
	let mut app = catalog();
	let action = Action::new(
		"large",
		"Generate a large update",
		|_: Input, context: InvocationContext<String>| async move {
			context.emit("x".repeat(1 << 20)).await.unwrap();
			Ok::<_, ()>(json!({}))
		},
	)
	.unwrap();
	app.register(Entry::new(action).mcp(StructuredBinding::serde_progress(NonZeroUsize::MIN)))
		.unwrap();
	let mut requests = ready();
	requests.extend([subscribed(2, "large", json!("large")), call(3, "echo")]);
	let frames = exchange(&app, &requests);
	assert_eq!(frames.len(), 3);
	assert_eq!(frames[1]["id"], 2);
	assert_eq!(frames[1]["error"]["code"], -32603);
	assert_eq!(frames[2]["id"], 3);
}

#[test]
fn a_failing_mapper_runs_only_for_subscribers_and_metadata_is_validated_before_execution() {
	let mut app = catalog();
	let invocations = Arc::new(AtomicUsize::new(0));
	let runs = invocations.clone();
	let action = Action::new(
		"mapped",
		"Emit an event",
		move |_: Input, context: InvocationContext<()>| {
			runs.fetch_add(1, Ordering::Relaxed);
			async move {
				context.emit(()).await.unwrap();
				Ok::<_, ()>(json!({"done":true}))
			}
		},
	)
	.unwrap();
	let binding = StructuredBinding::serde_with_progress(
		|_| Err("cannot map update".into()),
		NonZeroUsize::MIN,
	);
	app.register(Entry::new(action).mcp(binding)).unwrap();

	let mut requests = ready();
	requests.extend([
		request(
			2,
			"tools/call",
			json!({"name":"mapped", "_meta":{"trace":"abc"}}),
		),
		subscribed(3, "mapped", json!("mapped")),
		request(
			4,
			"tools/call",
			json!({"name":"mapped", "_meta":"malformed"}),
		),
	]);
	let frames = exchange(&app, &requests);
	assert_eq!(frames.len(), 4);
	assert_eq!(frames[1]["id"], 2);
	assert_eq!(
		frames[1]["result"]["structuredContent"],
		json!({"done":true})
	);
	assert_eq!(frames[2]["id"], 3);
	assert_eq!(frames[2]["error"]["code"], -32603);
	assert!(
		frames[2]["error"]["message"]
			.as_str()
			.unwrap()
			.contains("cannot map update")
	);
	assert_eq!(frames[3]["id"], 4);
	assert_eq!(frames[3]["error"]["code"], -32602);
	assert_eq!(invocations.load(Ordering::Relaxed), 2);
}
