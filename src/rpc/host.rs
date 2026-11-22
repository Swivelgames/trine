//! Connection admission, scheduling, and settlement. Wire presentation lives in `sink`.

mod admission;
mod writer;

use writer::write_messages;

use super::{
	Config,
	framing::{self, Frame, Frames},
	outbox::{Message, Outbox},
	protocol::{self, Id},
};
use crate::{Application, Cancellation, registry::DispatchReport};
use futures_lite::{
	future,
	io::{AsyncRead, AsyncWrite},
};
use serde_json::Value;
use std::{
	collections::{HashMap, HashSet, VecDeque},
	io,
};

/// Connection-wide counters and failure detail returned when an RPC session ends.
#[derive(Default, Debug)]
pub struct HostReport {
	/// Number of invocations admitted by the connection.
	pub invocations: u64,
	/// Number of rejections issued by the connection.
	pub rejected: u64,
	/// Connection failure diagnostic, or `None` after normal settlement.
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
