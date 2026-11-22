//! Resolve control messages and admit invocations before scheduling their handlers.

use super::{Active, Connection, Event};
use crate::{
	Application, Cancellation,
	application::InvocationError,
	error::{Failure, FailureKind},
	rpc::{
		protocol::{self, Id, Request},
		sink::Sink,
	},
};
use serde_json::{Value, json};
use std::sync::Arc;

impl Connection {
	fn duplicate(&self, id: &Id) -> Option<Value> {
		(self.active.contains_key(id) || self.known.contains(id)).then(|| {
			json!({
				"jsonrpc": "2.0",
				"method": "rejected",
				"params": {
					"code": "duplicate_id",
					"message": "Invocation ID has already been used",
					"data": { "id": id.value() },
				},
			})
		})
	}

	pub(super) fn dispatch(
		&mut self,
		request: Request,
		app: &Application,
		executor: &smol::LocalExecutor<'_>,
		events: &async_channel::Sender<Event>,
	) -> Option<Value> {
		let (id, path, input, observe) = match request {
			Request::Ignore => return None,
			Request::Reject(value) => {
				return Some(
					Id::parse(&value["id"], self.config.id_bytes)
						.and_then(|id| self.duplicate(&id))
						.unwrap_or(value),
				);
			}
			Request::Cancel(id) => {
				if let Some(entry) = self.active.get(&id)
					&& !entry.settled
				{
					entry.cancel.cancel();
				}
				return None;
			}
			Request::Invoke {
				id,
				path,
				input,
				observe,
			} => (id, path, input, observe),
		};

		if let Some(rejection) = self.duplicate(&id) {
			return Some(rejection);
		}
		let running = self.active.values().filter(|entry| !entry.settled).count();
		if running >= self.config.active_invocations {
			return Some(protocol::error(
				id.value(),
				-32000,
				"busy",
				"Invocation capacity occupied",
			));
		}

		let cancel = Cancellation::new();
		let sink = Arc::new(Sink::new(
			id.clone(),
			self.outbox.clone(),
			self.config.output_frame_bytes,
			cancel.clone(),
		));

		let preparation = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
			app.prepare_rpc_live(&path, input, observe, cancel.clone(), sink.clone())
		}))
		.unwrap_or_else(|_| {
			Err(InvocationError::Setup(Failure::message(
				FailureKind::Panicked,
				"view factory panicked",
			)))
		});
		let prepared = match preparation {
			Ok(prepared) => prepared,
			Err(error) => return Some(preparation_error(&id, error)),
		};

		let events = events.clone();
		let task_id = id.clone();
		let renderer = app.rpc_renderer();
		let task = executor.spawn(async move {
			let mut result = prepared.run().await;
			sink.framework(&mut result.execution, &renderer).await;
			let _ = events.send(Event::Settled(task_id, result.execution)).await;
		});

		self.active.insert(
			id,
			Active {
				cancel,
				task,
				settled: false,
				written: false,
			},
		);
		self.report.invocations += 1;
		None
	}
}

fn preparation_error(id: &Id, error: InvocationError) -> Value {
	let (code, kind) = match error {
		InvocationError::Unavailable => (-32602, "action_unavailable"),
		InvocationError::Input(_) => (-32602, "invalid_params"),
		InvocationError::Setup(_) => (-32603, "view_setup_failed"),
	};
	let mut rejection = protocol::error(
		id.value(),
		code,
		kind,
		"Invocation rejected before execution",
	);
	rejection["error"]["data"]["execution"] = json!("not_started");
	rejection
}
