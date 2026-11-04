//! Connection admission, scheduling, and settlement. Wire presentation lives in `sink`.

use super::{
	Config,
	framing::{self, Frame, Frames},
	outbox::{Message, Outbox},
	protocol::{self, Id, Request},
	sink::Sink,
};
use crate::{
	Application, Cancellation,
	application::InvocationError,
	error::{Failure, FailureKind},
	registry::DispatchReport,
};
use futures_lite::{
	future,
	io::{AsyncRead, AsyncWrite, AsyncWriteExt},
};
use serde_json::{Value, json};
use std::{
	collections::{HashMap, HashSet, VecDeque},
	io,
	sync::Arc,
};

#[derive(Default, Debug)]
pub struct HostReport {
	pub invocations: u64,
	pub rejected: u64,
	pub failure: Option<String>,
}

struct Active {
	cancel: Cancellation,
	task: smol::Task<()>,
	settled: bool,
	written: bool,
}

impl Active {
	fn can_retire(&self, delivery_abandoned: bool) -> bool {
		// Keep the ID reserved until execution and terminal delivery both settle.
		self.settled && (self.written || delivery_abandoned)
	}
}

enum Event {
	Settled(Id, DispatchReport),
	Written(Id),
	WriterFailed(String),
	WriterDone,
}

enum Input {
	Frame(io::Result<Frame>),
	Event(Result<Event, async_channel::RecvError>),
	Shutdown,
}

async fn write_messages<W: AsyncWrite + Unpin>(
	mut writer: W,
	outbox: Outbox,
	events: async_channel::Sender<Event>,
	stop: Cancellation,
) {
	while let Some(message) = outbox.next().await {
		let write = async {
			writer.write_all(&message.bytes).await?;
			writer.flush().await
		};

		let result = future::race(
			async {
				Some(
					crate::execution::supervise(write)
						.await
						.unwrap_or_else(|_| Err(io::Error::other("RPC writer panicked"))),
				)
			},
			async {
				stop.cancelled().await;
				None
			},
		)
		.await;
		match result {
			None => break,
			Some(Err(error)) => {
				let _ = events.send(Event::WriterFailed(error.to_string())).await;
				return;
			}
			Some(Ok(())) => {
				if let Some(id) = message.terminal {
					let _ = events.send(Event::Written(id)).await;
				}
			}
		}
	}

	let _ = events.send(Event::WriterDone).await;
}

struct Connection {
	config: Config,
	outbox: Outbox,
	active: HashMap<Id, Active>,
	history: VecDeque<Id>,
	known: HashSet<Id>,
	report: HostReport,
	reading: bool,
	stopping: bool,
	writer_done: bool,
}

impl Connection {
	async fn next_input<R: AsyncRead + Unpin>(
		&self,
		frames: &mut Frames<R>,
		incoming: &async_channel::Receiver<Event>,
		shutdown: &Cancellation,
	) -> Input {
		let read = async {
			if !self.reading {
				return future::pending().await;
			}
			Input::Frame(
				crate::execution::supervise(frames.next(self.config.input_frame_bytes))
					.await
					.unwrap_or_else(|_| Err(io::Error::other("RPC reader panicked"))),
			)
		};

		let control = async {
			if self.stopping {
				return future::pending().await;
			}
			shutdown.cancelled().await;
			Input::Shutdown
		};

		// Apply write/settlement receipts before admitting another ready request.
		future::or(
			control,
			future::or(async { Input::Event(incoming.recv().await) }, read),
		)
		.await
	}

	fn cancel(&mut self) {
		self.reading = false;
		self.stopping = true;
		for entry in self.active.values() {
			entry.cancel.cancel();
		}
	}

	fn fail(&mut self, error: String, abort_writer: &Cancellation) {
		self.report.failure = Some(error);
		self.cancel();
		self.outbox.close();
		abort_writer.cancel();
	}

	fn reject(&mut self, value: Value, abort_writer: &Cancellation) {
		self.report.rejected += 1;
		let result = framing::encode(&value, self.config.output_frame_bytes).and_then(|bytes| {
			self.outbox.reject(Message {
				bytes,
				terminal: None,
			})
		});

		if result.is_err() {
			self.fail("RPC rejection capacity exhausted".into(), abort_writer);
		}
	}

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

	fn dispatch(
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
		if self.active.values().filter(|entry| !entry.settled).count()
			>= self.config.active_invocations
		{
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
			Err(error) => {
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
				return Some(rejection);
			}
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

	fn receipt(
		&mut self,
		event: Event,
		settled: &mut impl FnMut(&DispatchReport),
		abort_writer: &Cancellation,
	) {
		match event {
			Event::Settled(id, result) => {
				if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| settled(&result)))
					.is_err()
				{
					self.report.failure = Some("RPC settlement observer panicked".into());
					self.cancel();
				}
				if let Some(entry) = self.active.get_mut(&id) {
					entry.settled = true;
				}
			}
			Event::Written(id) => {
				if let Some(entry) = self.active.get_mut(&id) {
					entry.written = true;
				}
			}
			Event::WriterFailed(error) => {
				self.writer_done = true;
				self.fail(error, abort_writer);
			}
			Event::WriterDone => self.writer_done = true,
		}
	}

	async fn retire(&mut self) {
		let finished: Vec<_> = self
			.active
			.iter()
			.filter(|(_, entry)| entry.can_retire(self.report.failure.is_some() && self.stopping))
			.map(|(id, _)| id.clone())
			.collect();

		for id in finished {
			self.active.remove(&id).unwrap().task.await;
			self.known.insert(id.clone());
			self.history.push_back(id);
			if self.history.len() > self.config.history {
				self.known.remove(&self.history.pop_front().unwrap());
			}
		}
	}
}

/// Serve one connection using cancellation-safe asynchronous reader/writer adapters.
/// The observer inspects settlement without retaining an unbounded outcome history.
pub async fn serve<R, W, F>(
	app: &Application,
	reader: R,
	writer: W,
	config: Config,
	shutdown: Cancellation,
	mut settled: F,
) -> io::Result<HostReport>
where
	R: AsyncRead + Unpin,
	W: AsyncWrite + Unpin,
	F: FnMut(&DispatchReport),
{
	config.validate()?;
	let executor = smol::LocalExecutor::new();
	executor
		.run(async {
			let outbox = Outbox::new(&config);
			let capacity = config.active_invocations + config.output_messages + 2;
			let (events, incoming) = async_channel::bounded(capacity);
			let abort_writer = Cancellation::new();
			let writer_task = executor.spawn(write_messages(
				writer,
				outbox.clone(),
				events.clone(),
				abort_writer.clone(),
			));

			let mut frames = Frames::new(reader);
			let mut connection = Connection {
				config,
				outbox,
				active: HashMap::new(),
				history: VecDeque::new(),
				known: HashSet::new(),
				report: HostReport::default(),
				reading: true,
				stopping: false,
				writer_done: false,
			};

			loop {
				if !connection.reading && connection.active.is_empty() {
					connection.outbox.finish();
				}
				if connection.writer_done && connection.active.is_empty() {
					break;
				}

				let event = connection
					.next_input(&mut frames, &incoming, &shutdown)
					.await;
				match event {
					Input::Shutdown => connection.cancel(),
					Input::Frame(Ok(Frame::Eof)) => connection.reading = false,
					Input::Frame(Err(error)) => {
						connection.report.failure = Some(error.to_string());
						connection.cancel();
					}
					Input::Frame(Ok(Frame::Oversized)) => connection.reject(
						protocol::error(
							Value::Null,
							-32001,
							"frame_too_large",
							"Input frame exceeds limit",
						),
						&abort_writer,
					),
					Input::Frame(Ok(Frame::Data(bytes))) => {
						let request = protocol::decode(&bytes, connection.config.id_bytes);
						if let Some(rejection) =
							connection.dispatch(request, app, &executor, &events)
						{
							connection.reject(rejection, &abort_writer);
						}
					}
					Input::Event(Ok(event)) => {
						connection.receipt(event, &mut settled, &abort_writer)
					}
					Input::Event(Err(_)) => {
						connection.fail("RPC event channel closed".into(), &abort_writer)
					}
				}

				connection.retire().await;
				future::yield_now().await;
			}

			writer_task.await;
			Ok(connection.report)
		})
		.await
}
