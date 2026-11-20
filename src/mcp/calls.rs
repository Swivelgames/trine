//! Own each running invocation until it settles, including after disconnect.

use super::{delivery::Output, progress::Progress, protocol, tool};
use crate::{Application, Cancellation, execution, jsonrpc::Id};
use serde_json::Value;
use std::{
	future::{Future, poll_fn},
	pin::Pin,
	sync::Arc,
	task::Poll,
};

const MAX_CALLS: usize = 16;
const RESERVED_OBSERVATIONS: usize = 2;

struct Call {
	id: Id,
	observation: bool,
	cancel: Cancellation,
	withdrawn: bool,
	progress: Option<Arc<Progress>>,
	reply: Pin<Box<dyn Future<Output = Value> + Send>>,
}

impl Call {
	fn withdraw(&mut self) {
		self.withdrawn = true;
		if let Some(progress) = &self.progress {
			progress.withdraw();
		}
		self.cancel.cancel();
	}
}

#[derive(Default)]
pub(super) struct Calls(Vec<Call>);

impl Calls {
	pub fn contains(&self, id: &Id) -> bool {
		self.0.iter().any(|call| call.id == *id)
	}

	fn has_token(&self, token: &Id) -> bool {
		self.0
			.iter()
			.filter_map(|call| call.progress.as_ref())
			.any(|progress| progress.token == *token)
	}

	fn has_capacity(&self, observation: bool) -> bool {
		if self.0.len() >= MAX_CALLS {
			return false;
		}

		let mutations = self.0.iter().filter(|call| !call.observation).count();
		observation || mutations < MAX_CALLS - RESERVED_OBSERVATIONS
	}

	pub fn start(
		&mut self,
		app: &Application,
		id: Id,
		name: &str,
		arguments: Value,
		token: Option<Id>,
		output: &Output,
	) -> Option<Value> {
		if token.as_ref().is_some_and(|token| self.has_token(token)) {
			return Some(protocol::error(
				id.value(),
				-32602,
				"Progress token already active",
			));
		}

		let path: Vec<_> = name.split('.').map(str::to_owned).collect();
		let Some(tool) = app.mcp_tool(&path) else {
			return Some(protocol::error(id.value(), -32602, "Unknown tool"));
		};
		let observation = tool.observation;
		if !self.has_capacity(observation) {
			return Some(protocol::error(
				id.value(),
				-32000,
				"Tool call capacity occupied",
			));
		}

		let cancel = Cancellation::new();
		let progress =
			token.map(|token| Arc::new(Progress::new(token, output.clone(), cancel.clone())));
		let prepared = match tool::prepare(tool, &id, arguments, cancel.clone(), progress.clone()) {
			Ok(prepared) => prepared,
			Err(response) => {
				if let Some(progress) = progress {
					progress.withdraw();
					progress.close();
				}
				return Some(response);
			}
		};

		let reply_id = id.clone();
		let reply = async move {
			match execution::supervise(prepared.run()).await {
				Ok(report) => tool::outcome(&reply_id, report),
				Err(_) => protocol::error(reply_id.value(), -32603, "MCP tool execution panicked"),
			}
		};
		self.0.push(Call {
			id,
			observation,
			cancel,
			withdrawn: false,
			progress,
			reply: Box::pin(reply),
		});
		None
	}

	pub fn cancel(&mut self, id: &Id) {
		if let Some(call) = self.0.iter_mut().find(|call| call.id == *id) {
			call.withdraw();
		}
	}

	/// Poll all admitted calls on the host executor. Dropping this wait leaves
	/// the individual futures owned by the connection.
	pub async fn next(&mut self) -> Option<Value> {
		poll_fn(|cx| {
			for (index, call) in self.0.iter_mut().enumerate() {
				if let Poll::Ready(reply) = call.reply.as_mut().poll(cx) {
					let call = self.0.swap_remove(index);
					if let Some(progress) = &call.progress {
						progress.close();
					}
					return Poll::Ready((!call.withdrawn).then_some(reply));
				}
			}
			Poll::Pending
		})
		.await
	}

	pub fn withdraw_all(&mut self) {
		for call in &mut self.0 {
			call.withdraw();
		}
	}

	pub async fn settle(mut self) {
		while !self.0.is_empty() {
			self.next().await;
		}
	}
}
