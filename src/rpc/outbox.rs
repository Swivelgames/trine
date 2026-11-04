//! Bounded output queues keep invocation traffic separate from request rejections.

use super::{Config, protocol::Id};
use crate::{
	Cancellation,
	error::{Failure, FailureKind},
};
use std::{
	collections::VecDeque,
	sync::{Arc, Mutex},
};

pub(crate) struct Message {
	pub bytes: Vec<u8>,
	pub terminal: Option<Id>,
}

struct Queue {
	messages: VecDeque<Message>,
	queued_bytes: usize,
	message_limit: usize,
	byte_limit: usize,
}

impl Queue {
	fn new(message_limit: usize, byte_limit: usize) -> Self {
		Self {
			messages: VecDeque::new(),
			queued_bytes: 0,
			message_limit,
			byte_limit,
		}
	}

	fn fits(&self, size: usize) -> bool {
		self.messages.len() < self.message_limit
			&& size <= self.byte_limit.saturating_sub(self.queued_bytes)
	}

	fn push(&mut self, message: Message) {
		self.queued_bytes += message.bytes.len();
		self.messages.push_back(message);
	}

	fn pop(&mut self) -> Option<Message> {
		let message = self.messages.pop_front()?;
		self.queued_bytes -= message.bytes.len();
		Some(message)
	}
}

struct State {
	ordinary: Queue,
	rejection: Queue,
	changed: Cancellation,
	closed: bool,
	finishing: bool,
	rejection_turn: bool,
}

impl State {
	fn notify_waiters(&mut self) {
		// Replace the signal while locked so new waiters observe the next change.
		std::mem::take(&mut self.changed).cancel();
	}
}

#[derive(Clone)]
pub(crate) struct Outbox(Arc<Mutex<State>>);

impl Outbox {
	pub fn new(config: &Config) -> Self {
		Self(Arc::new(Mutex::new(State {
			ordinary: Queue::new(config.output_messages, config.output_bytes),
			rejection: Queue::new(config.rejection_messages, config.rejection_bytes),
			changed: Cancellation::new(),
			closed: false,
			finishing: false,
			rejection_turn: true,
		})))
	}

	/// Wait for room; return false if cancellation wins before admission.
	pub async fn send(
		&self,
		message: Message,
		cancellation: Option<&Cancellation>,
	) -> Result<bool, Failure> {
		loop {
			let changed = {
				let mut state = self.0.lock().unwrap();

				if state.closed || state.finishing {
					return Err(delivery_closed());
				}

				if cancellation.is_some_and(Cancellation::is_cancelled) {
					return Ok(false);
				}

				if state.ordinary.fits(message.bytes.len()) {
					state.ordinary.push(message);
					state.notify_waiters();
					return Ok(true);
				}

				state.changed.clone()
			};

			// Never hold the queue lock while waiting for capacity or cancellation.
			if let Some(cancel) = cancellation {
				futures_lite::future::race(changed.cancelled(), cancel.cancelled()).await;
			} else {
				changed.cancelled().await;
			}
		}
	}

	/// Reject without waiting, keeping the request reader responsive.
	pub fn reject(&self, message: Message) -> Result<(), Failure> {
		let mut state = self.0.lock().unwrap();
		if state.closed || !state.rejection.fits(message.bytes.len()) {
			return Err(delivery_closed());
		}

		state.rejection.push(message);
		state.notify_waiters();
		Ok(())
	}

	pub async fn next(&self) -> Option<Message> {
		loop {
			let changed = {
				let mut state = self.0.lock().unwrap();

				if state.closed {
					return None;
				}

				let message = if state.rejection_turn {
					state.rejection.pop().or_else(|| state.ordinary.pop())
				} else {
					state.ordinary.pop().or_else(|| state.rejection.pop())
				};
				state.rejection_turn = !state.rejection_turn;

				if message.is_some() {
					state.notify_waiters();
					return message;
				}

				if state.finishing {
					return None;
				}

				state.changed.clone()
			};

			changed.cancelled().await;
		}
	}

	/// Stop delivery immediately and discard queued messages.
	pub fn close(&self) {
		let mut state = self.0.lock().unwrap();
		state.closed = true;
		state.ordinary.messages.clear();
		state.rejection.messages.clear();
		state.notify_waiters();
	}

	/// Stop accepting ordinary messages and let the writer drain the queues.
	pub fn finish(&self) {
		let mut state = self.0.lock().unwrap();
		state.finishing = true;
		state.notify_waiters();
	}
}

fn delivery_closed() -> Failure {
	Failure::message(FailureKind::OutputFailed, "RPC delivery closed")
}
