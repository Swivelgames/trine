//! Live RPC host behavior under cancellation and bounded delivery.

use futures_lite::io::{AsyncRead, AsyncWrite, Cursor};
use serde_json::{Value, json};
use std::{
	future::Future,
	io,
	num::NonZeroUsize,
	pin::Pin,
	sync::{
		Arc, Mutex,
		atomic::{AtomicBool, Ordering},
	},
	task::{Context, Poll},
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext,
	rpc::{self, Config},
	structured::StructuredBinding,
};

struct Wait {
	signal: Box<Cancellation>,
	waiting: Pin<Box<dyn Future<Output = ()>>>,
}
impl Wait {
	fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
		if self.signal.is_cancelled() {
			return Poll::Ready(());
		}
		self.waiting.as_mut().poll(cx)
	}
}
fn wait(signal: Cancellation) -> Wait {
	let waiting = signal.clone();
	Wait {
		signal: Box::new(signal),
		waiting: Box::pin(async move { waiting.cancelled().await }),
	}
}
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl AsyncWrite for Output {
	fn poll_write(
		self: Pin<&mut Self>,
		_: &mut Context<'_>,
		bytes: &[u8],
	) -> Poll<io::Result<usize>> {
		self.0.lock().unwrap().extend_from_slice(bytes);
		Poll::Ready(Ok(bytes.len()))
	}
	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
	fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
}
struct GateWriter {
	output: Output,
	ready: Wait,
}
impl AsyncWrite for GateWriter {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &[u8],
	) -> Poll<io::Result<usize>> {
		if self.ready.poll(cx).is_pending() {
			return Poll::Pending;
		}
		Pin::new(&mut self.output).poll_write(cx, bytes)
	}
	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
	fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
}
struct PausedReader {
	input: Cursor<Vec<u8>>,
	boundary: u64,
	ready: Wait,
}
impl AsyncRead for PausedReader {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &mut [u8],
	) -> Poll<io::Result<usize>> {
		if self.input.position() >= self.boundary && self.ready.poll(cx).is_pending() {
			return Poll::Pending;
		}
		let remaining = self.boundary.saturating_sub(self.input.position()) as usize;
		let length = if remaining == 0 {
			bytes.len()
		} else {
			bytes.len().min(remaining)
		};
		Pin::new(&mut self.input).poll_read(cx, &mut bytes[..length])
	}
}
fn invoke(id: u64) -> String {
	format!(
		"{}\n",
		json!({"jsonrpc":"2.0","id":id,"method":"invoke","params":{"action":["run"],"input":null,"observe":true}})
	)
}
fn app(action: Action<(), bool, (), u64>) -> Application {
	let mut app = Application::new("test", "1").unwrap();
	app.register(
		Entry::new(action).rpc(StructuredBinding::serde_with_progress(
			|value| Ok(json!(value)),
			NonZeroUsize::new(1).unwrap(),
		)),
	)
	.unwrap();
	app
}
fn bounded() -> Config {
	Config {
		output_messages: 1,
		output_frame_bytes: 2048,
		output_bytes: 4096,
		id_bytes: 32,
		..Config::default()
	}
}

#[test]
fn cancellation_reaches_handler_while_output_is_full() {
	let reader_ready = Cancellation::new();
	let writer_ready = Cancellation::new();
	let read_signal = reader_ready.clone();
	let write_signal = writer_ready.clone();
	let cancelled = Arc::new(AtomicBool::new(false));
	let observed = cancelled.clone();
	let action = Action::new(
		"run",
		"Run",
		move |_: (), context: InvocationContext<u64>| {
			let read_signal = read_signal.clone();
			let write_signal = write_signal.clone();
			let observed = observed.clone();
			async move {
				for index in 0..100 {
					if index == 4 {
						read_signal.cancel();
					}
					if context.emit(index).await.is_err() {
						break;
					}
				}
				context.cancellation().cancelled().await;
				observed.store(true, Ordering::SeqCst);
				write_signal.cancel();
				Ok(true)
			}
		},
	)
	.unwrap();
	let first = invoke(1);
	let input = format!(
		"{first}{}\n",
		json!({"jsonrpc":"2.0","method":"cancel","params":{"id":1}})
	);
	let reader = PausedReader {
		input: Cursor::new(input.into_bytes()),
		boundary: first.len() as u64,
		ready: wait(reader_ready),
	};
	let output = Output::default();
	let capture = output.clone();
	let writer = GateWriter {
		output,
		ready: wait(writer_ready),
	};
	let result = smol::block_on(rpc::serve(
		&app(action),
		reader,
		writer,
		bounded(),
		Cancellation::new(),
		|_| {},
	))
	.unwrap();
	assert!(cancelled.load(Ordering::SeqCst));
	assert!(result.failure.is_none());
	let bytes = capture.0.lock().unwrap();
	let frames: Vec<Value> = bytes
		.split(|b| *b == b'\n')
		.filter(|b| !b.is_empty())
		.map(|b| serde_json::from_slice(b).unwrap())
		.collect();
	assert_eq!(frames.last().unwrap()["result"], true);
	for (index, frame) in frames[..frames.len() - 1].iter().enumerate() {
		assert_eq!(frame["params"]["sequence"], index + 1);
	}
}

struct BrokenWriter;
impl AsyncWrite for BrokenWriter {
	fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
		Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken")))
	}
	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
	fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
}
#[test]
fn output_failure_cancels_and_observes_owned_settlement() {
	let settled = Arc::new(AtomicBool::new(false));
	let done = settled.clone();
	let action = Action::new(
		"run",
		"Run",
		move |_: (), context: InvocationContext<u64>| {
			let done = done.clone();
			async move {
				let _ = context.emit(1).await;
				context.cancellation().cancelled().await;
				done.store(true, Ordering::SeqCst);
				Ok(true)
			}
		},
	)
	.unwrap();
	let mut observed = false;
	let report = smol::block_on(rpc::serve(
		&app(action),
		Cursor::new(invoke(1).into_bytes()),
		BrokenWriter,
		bounded(),
		Cancellation::new(),
		|report| {
			observed = true;
			assert!(report.domain_succeeded);
		},
	))
	.unwrap();
	assert!(report.failure.is_some());
	assert!(observed);
	assert!(settled.load(Ordering::SeqCst));
}
#[test]
fn rejection_saturation_stops_delivery_and_settles_actions() {
	let settled = Arc::new(AtomicBool::new(false));
	let done = settled.clone();
	let action = Action::new(
		"run",
		"Run",
		move |_: (), context: InvocationContext<u64>| {
			let done = done.clone();
			async move {
				context.cancellation().cancelled().await;
				done.store(true, Ordering::SeqCst);
				Ok(true)
			}
		},
	)
	.unwrap();
	let input = invoke(1) + &"invalid\n".repeat(100);
	let writer = GateWriter {
		output: Output::default(),
		ready: wait(Cancellation::new()),
	};
	let config = Config {
		rejection_messages: 100,
		rejection_bytes: 512,
		..bounded()
	};
	let report = smol::block_on(rpc::serve(
		&app(action),
		Cursor::new(input.into_bytes()),
		writer,
		config,
		Cancellation::new(),
		|_| {},
	))
	.unwrap();
	assert!(report.failure.unwrap().contains("rejection capacity"));
	assert!(settled.load(Ordering::SeqCst));
}
struct ErrorAfter {
	input: Cursor<Vec<u8>>,
}
impl AsyncRead for ErrorAfter {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &mut [u8],
	) -> Poll<io::Result<usize>> {
		if self.input.position() == self.input.get_ref().len() as u64 {
			return Poll::Ready(Err(io::Error::other("input failed")));
		}
		Pin::new(&mut self.input).poll_read(cx, bytes)
	}
}
#[test]
fn input_failure_cancels_but_retains_usable_output() {
	let action = Action::new(
		"run",
		"Run",
		|_: (), context: InvocationContext<u64>| async move {
			context.cancellation().cancelled().await;
			Ok(true)
		},
	)
	.unwrap();
	let output = Output::default();
	let capture = output.clone();
	let report = smol::block_on(rpc::serve(
		&app(action),
		ErrorAfter {
			input: Cursor::new(invoke(1).into_bytes()),
		},
		output,
		bounded(),
		Cancellation::new(),
		|_| {},
	))
	.unwrap();
	assert!(report.failure.unwrap().contains("input failed"));
	let bytes = capture.0.lock().unwrap();
	let value: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(value["result"], true);
}

#[test]
fn busy_does_not_consume_cancellation_capacity() {
	let action = Action::new(
		"run",
		"Run",
		|_: (), context: InvocationContext<u64>| async move {
			context.cancellation().cancelled().await;
			Ok(true)
		},
	)
	.unwrap();
	let input = invoke(1)
		+ &invoke(2)
		+ &format!(
			"{}\n",
			json!({"jsonrpc":"2.0","method":"cancel","params":{"id":1}})
		);
	let output = Output::default();
	let capture = output.clone();
	let config = Config {
		active_invocations: 1,
		..bounded()
	};
	let report = smol::block_on(rpc::serve(
		&app(action),
		Cursor::new(input.into_bytes()),
		output,
		config,
		Cancellation::new(),
		|_| {},
	))
	.unwrap();
	assert_eq!(report.invocations, 1);
	let bytes = capture.0.lock().unwrap();
	let frames: Vec<Value> = bytes
		.split(|b| *b == b'\n')
		.filter(|b| !b.is_empty())
		.map(|b| serde_json::from_slice(b).unwrap())
		.collect();
	assert!(
		frames
			.iter()
			.any(|v| v["id"] == 2 && v["error"]["data"]["code"] == "busy")
	);
	assert!(frames.iter().any(|v| v["id"] == 1 && v["result"] == true));
}

#[cfg(unix)]
#[test]
fn recent_history_rejects_then_evicts_settled_ids() {
	use futures_lite::io::{AsyncReadExt, AsyncWriteExt};
	smol::block_on(async {
		let action = Action::new("run", "Run", |_: (), _: InvocationContext<u64>| async {
			Ok(true)
		})
		.unwrap();
		let app = app(action);
		let (server, mut client) = smol::net::unix::UnixStream::pair().unwrap();
		let config = Config {
			history: 1,
			..bounded()
		};
		let host = rpc::serve(
			&app,
			server.clone(),
			server,
			config,
			Cancellation::new(),
			|_| {},
		);
		let exercise = async {
			for (id, duplicate) in [(1, false), (1, true), (2, false), (1, false)] {
				let request = if duplicate {
					format!(
						"{}\n",
						json!({"jsonrpc":"2.0","id":id,"method":"invoke","params":{}})
					)
				} else {
					invoke(id)
				};
				client.write_all(request.as_bytes()).await.unwrap();
				let mut frame = Vec::new();
				loop {
					let mut byte = [0];
					client.read_exact(&mut byte).await.unwrap();
					if byte[0] == b'\n' {
						break;
					}
					frame.push(byte[0]);
				}
				let value: Value = serde_json::from_slice(&frame).unwrap();
				if duplicate {
					assert_eq!(value["method"], "rejected");
				} else {
					assert_eq!(value["result"], true);
				}
			}
			client.shutdown(std::net::Shutdown::Write).unwrap();
		};
		let (report, ()) = futures_lite::future::zip(host, exercise).await;
		assert_eq!(report.unwrap().invocations, 3);
	});
}

struct FlushFailure(Output);
impl AsyncWrite for FlushFailure {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &[u8],
	) -> Poll<io::Result<usize>> {
		Pin::new(&mut self.0).poll_write(cx, bytes)
	}
	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Err(io::Error::other("flush failed")))
	}
	fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}
}
#[test]
fn terminal_flush_failure_does_not_send_a_second_response() {
	let action = Action::new("run", "Run", |_: (), _: InvocationContext<u64>| async {
		Ok(true)
	})
	.unwrap();
	let output = Output::default();
	let capture = output.clone();
	let report = smol::block_on(rpc::serve(
		&app(action),
		Cursor::new(invoke(1).into_bytes()),
		FlushFailure(output),
		bounded(),
		Cancellation::new(),
		|r| assert!(r.domain_succeeded),
	))
	.unwrap();
	assert!(report.failure.unwrap().contains("flush failed"));
	let bytes = capture.0.lock().unwrap();
	let value: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(value["result"], true);
}

#[test]
fn observer_panic_cancels_and_settles_other_owned_actions() {
	use std::sync::atomic::AtomicUsize;
	let calls = Arc::new(AtomicUsize::new(0));
	let second_started = Cancellation::new();
	let action = Action::new(
		"run",
		"Run",
		move |_: (), context: InvocationContext<u64>| {
			let index = calls.fetch_add(1, Ordering::SeqCst);
			let second_started = second_started.clone();
			async move {
				if index == 0 {
					second_started.cancelled().await;
				} else {
					second_started.cancel();
					context.cancellation().cancelled().await;
				}
				Ok(true)
			}
		},
	)
	.unwrap();
	let mut settlements = 0;
	let report = smol::block_on(rpc::serve(
		&app(action),
		Cursor::new((invoke(1) + &invoke(2)).into_bytes()),
		Output::default(),
		bounded(),
		Cancellation::new(),
		|result| {
			settlements += 1;
			assert!(result.domain_succeeded);
			if settlements == 1 {
				panic!("observer failed");
			}
		},
	))
	.unwrap();
	assert_eq!(settlements, 2);
	assert!(report.failure.unwrap().contains("observer panicked"));
}

#[test]
fn malformed_duplicate_cannot_replace_active_response() {
	let action = Action::new(
		"run",
		"Run",
		|_: (), context: InvocationContext<u64>| async move {
			context.cancellation().cancelled().await;
			Ok(true)
		},
	)
	.unwrap();
	let input = format!(
		"{}{}\n{}\n",
		invoke(1),
		json!({"jsonrpc":"2.0","id":1,"method":"invoke","params":{}}),
		json!({"jsonrpc":"2.0","method":"cancel","params":{"id":1}}),
	);
	let output = Output::default();
	let capture = output.clone();
	let report = smol::block_on(rpc::serve(
		&app(action),
		Cursor::new(input.into_bytes()),
		output,
		bounded(),
		Cancellation::new(),
		|_| {},
	))
	.unwrap();
	assert_eq!(report.invocations, 1);
	let bytes = capture.0.lock().unwrap();
	let frames: Vec<Value> = bytes
		.split(|b| *b == b'\n')
		.filter(|b| !b.is_empty())
		.map(|b| serde_json::from_slice(b).unwrap())
		.collect();
	assert_eq!(frames.iter().filter(|v| v.get("id").is_some()).count(), 1);
	assert!(frames.iter().any(|v| v["id"] == 1 && v["result"] == true));
	assert!(frames.iter().any(|v| v["method"] == "rejected"));
}

struct EndlessReader {
	prefix: Cursor<Vec<u8>>,
	bytes: Arc<std::sync::atomic::AtomicUsize>,
	limit_reached: Box<Cancellation>,
}
impl AsyncRead for EndlessReader {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &mut [u8],
	) -> Poll<io::Result<usize>> {
		if self.prefix.position() < self.prefix.get_ref().len() as u64 {
			return Pin::new(&mut self.prefix).poll_read(cx, bytes);
		}
		bytes.fill(b'x');
		if self.bytes.fetch_add(bytes.len(), Ordering::SeqCst) >= 32768 {
			self.limit_reached.cancel();
		}
		Poll::Ready(Ok(bytes.len()))
	}
}
#[test]
fn endless_ready_oversized_input_yields_to_shutdown_and_owned_settlement() {
	let limit_reached = Cancellation::new();
	let shutdown = Cancellation::new();
	let action = Action::new(
		"run",
		"Run",
		|_: (), context: InvocationContext<u64>| async move {
			context.cancellation().cancelled().await;
			Ok(true)
		},
	)
	.unwrap();
	let reader = EndlessReader {
		prefix: Cursor::new(invoke(1).into_bytes()),
		bytes: Arc::default(),
		limit_reached: Box::new(limit_reached.clone()),
	};
	let config = Config {
		input_frame_bytes: 256,
		..bounded()
	};
	let mut settled = false;
	smol::block_on(async {
		let application = app(action);
		let host = rpc::serve(
			&application,
			reader,
			Output::default(),
			config,
			shutdown.clone(),
			|r| {
				assert!(r.domain_succeeded);
				settled = true;
			},
		);
		let control = async {
			limit_reached.cancelled().await;
			shutdown.cancel();
		};
		let (report, ()) = futures_lite::future::zip(host, control).await;
		assert_eq!(report.unwrap().invocations, 1);
	});
	assert!(settled);
}

#[test]
fn ordinary_byte_budget_backpressures_with_spare_message_slots_then_resumes() {
	let pressure = Cancellation::new();
	let ready = Cancellation::new();
	let done = Arc::new(AtomicBool::new(false));
	let completed = done.clone();
	let reached = pressure.clone();
	let action = Action::new(
		"run",
		"Run",
		move |_: (), context: InvocationContext<u64>| {
			let completed = completed.clone();
			let reached = reached.clone();
			async move {
				for index in 0..20 {
					if index == 5 {
						reached.cancel();
					}
					context.emit(index).await.unwrap();
				}
				completed.store(true, Ordering::SeqCst);
				Ok::<_, ()>(true)
			}
		},
	)
	.unwrap();
	let mut application = Application::new("test", "1").unwrap();
	application
		.register(
			Entry::new(action).rpc(StructuredBinding::serde_with_progress(
				|index| Ok(json!({"index":index,"padding":"x".repeat(1400)})),
				NonZeroUsize::new(1).unwrap(),
			)),
		)
		.unwrap();
	let output = Output::default();
	let capture = output.clone();
	let writer = GateWriter {
		output,
		ready: wait(ready.clone()),
	};
	let config = Config {
		output_messages: 64,
		..bounded()
	};
	smol::block_on(async {
		let host = rpc::serve(
			&application,
			Cursor::new(invoke(1).into_bytes()),
			writer,
			config,
			Cancellation::new(),
			|_| {},
		);
		let release = async {
			pressure.cancelled().await;
			for _ in 0..128 {
				futures_lite::future::yield_now().await;
			}
			assert!(
				!done.load(Ordering::SeqCst),
				"byte budget must block before all events fit"
			);
			ready.cancel();
		};
		let (report, ()) = futures_lite::future::zip(host, release).await;
		assert!(report.unwrap().failure.is_none());
	});
	assert!(done.load(Ordering::SeqCst));
	let bytes = capture.0.lock().unwrap();
	let frames: Vec<Value> = bytes
		.split(|b| *b == b'\n')
		.filter(|b| !b.is_empty())
		.map(|b| serde_json::from_slice(b).unwrap())
		.collect();
	assert_eq!(frames.len(), 21);
	for (index, frame) in frames[..20].iter().enumerate() {
		assert_eq!(frame["params"]["sequence"], index + 1);
	}
	assert_eq!(frames[20]["result"], true);
}
