//! Concurrent tool admission with capacity reserved for read-only calls.

use super::fixture::*;
use serde_json::{Value, json};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, structured::StructuredBinding,
};

fn waiting_snapshot(id: u32) -> Value {
	request(
		id,
		"tools/call",
		json!({"name":"snapshot","arguments":{"value":"wait"}}),
	)
}

fn catalog_with_observation() -> Application {
	let mut app = waiting(Cancellation::new(), Cancellation::new());
	let snapshot = Action::new(
		"snapshot",
		"Observe current state",
		|input: Input, context: InvocationContext<()>| async move {
			if input.value.as_deref() == Some("wait") {
				context.cancellation().cancelled().await;
			}
			Ok::<_, ()>(json!({"state":"ready"}))
		},
	)
	.unwrap();
	app.register(
		Entry::new(snapshot)
			.mcp(StructuredBinding::serde())
			.mcp_observation(),
	)
	.unwrap();

	app
}

#[test]
fn mutations_leave_bounded_room_for_observations_and_control_on_the_same_connection() {
	let app = catalog_with_observation();
	let mut requests = ready();
	requests.extend((2..=16).map(|id| call(id, "wait")));
	requests.push(call(17, "snapshot"));
	for id in 18..=20 {
		requests.push(waiting_snapshot(id));
	}
	requests.push(request(21, "ping", json!({})));
	requests.push(notification(
		"notifications/cancelled",
		json!({"requestId":18}),
	));
	requests.push(call(22, "snapshot"));

	let replies = exchange(&app, &requests);
	assert_eq!(replies.len(), 6);
	assert_eq!(replies[1]["id"], 16);
	assert_eq!(replies[1]["error"]["code"], -32000);
	assert_eq!(replies[2]["id"], 17);
	assert_eq!(
		replies[2]["result"]["structuredContent"],
		json!({"state":"ready"})
	);
	assert_eq!(replies[3]["id"], 20);
	assert_eq!(replies[3]["error"]["code"], -32000);
	assert_eq!(replies[4]["id"], 21);
	assert_eq!(replies[4]["result"], json!({}));
	assert_eq!(replies[5]["id"], 22);
	assert_eq!(
		replies[5]["result"]["structuredContent"],
		json!({"state":"ready"})
	);
}

#[test]
fn two_active_observations_leave_fourteen_slots_for_mutations() {
	let app = catalog_with_observation();
	let mut requests = ready();
	requests.extend([waiting_snapshot(2), waiting_snapshot(3)]);
	requests.extend((4..=18).map(|id| call(id, "wait")));

	let replies = exchange(&app, &requests);
	assert_eq!(replies.len(), 2);
	assert_eq!(replies[1]["id"], 18);
	assert_eq!(replies[1]["error"]["code"], -32000);
}

#[test]
fn observation_only_traffic_can_use_all_sixteen_slots() {
	let app = catalog_with_observation();
	let mut requests = ready();
	requests.extend((2..=18).map(waiting_snapshot));

	let replies = exchange(&app, &requests);
	assert_eq!(replies.len(), 2);
	assert_eq!(replies[1]["id"], 18);
	assert_eq!(replies[1]["error"]["code"], -32000);
}
