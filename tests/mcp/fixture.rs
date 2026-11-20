use futures_lite::io::{AsyncWrite, Cursor};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
	future::Future,
	io,
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};
use trine::{
	Action, Application, Cancellation, Entry, InvocationContext, mcp, structured::StructuredBinding,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
	pub(super) value: Option<String>,
}

pub(super) fn request(id: u32, method: &str, params: Value) -> Value {
	json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub(super) fn notification(method: &str, params: Value) -> Value {
	json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

pub(super) fn initialize() -> Value {
	request(
		1,
		"initialize",
		json!({
			"protocolVersion": "other-revision", "capabilities": {},
			"clientInfo": { "name": "test-client", "version": "1" }
		}),
	)
}

pub(super) fn ready() -> Vec<Value> {
	vec![
		initialize(),
		notification("notifications/initialized", json!({})),
	]
}

pub(super) fn call(id: u32, name: &str) -> Value {
	request(id, "tools/call", json!({ "name": name }))
}

pub(super) fn subscribed(id: u32, tool: &str, token: Value) -> Value {
	request(
		id,
		"tools/call",
		json!({ "name":tool, "_meta":{"progressToken":token} }),
	)
}

pub(super) fn lines(requests: &[Value]) -> Vec<u8> {
	let mut bytes = Vec::new();
	for message in requests {
		serde_json::to_writer(&mut bytes, message).unwrap();
		bytes.push(b'\n');
	}
	bytes
}

pub(super) fn replies(output: &[u8]) -> Vec<Value> {
	std::str::from_utf8(output)
		.unwrap()
		.lines()
		.map(|line| serde_json::from_str(line).unwrap())
		.collect()
}

pub(super) fn serve(
	app: &Application,
	input: Vec<u8>,
	output: impl AsyncWrite + Unpin,
	shutdown: Cancellation,
) -> io::Result<()> {
	run(mcp::serve(app, Cursor::new(input), output, shutdown))
}

pub(super) fn run<T>(work: impl Future<Output = T>) -> T {
	smol::block_on(futures_lite::future::or(work, async {
		smol::Timer::after(Duration::from_secs(5)).await;
		panic!("host failed to settle")
	}))
}

pub(super) fn exchange(app: &Application, requests: &[Value]) -> Vec<Value> {
	let mut bytes = Vec::new();
	serve(app, lines(requests), &mut bytes, Cancellation::new()).unwrap();
	replies(&bytes)
}

pub(super) fn catalog() -> Application {
	let mut app = Application::new("example", "2.3").unwrap();
	let echo = Action::new(
		"echo",
		"Echo the input",
		|input: Input, _: InvocationContext<()>| async move {
			Ok::<_, ()>(json!({ "value": input.value }))
		},
	)
	.unwrap();
	let hidden = Entry::new(echo.clone())
		.namespace(["hidden"])
		.rpc(StructuredBinding::serde());
	app.register(hidden).unwrap();
	app.register(Entry::new(echo).mcp(StructuredBinding::serde()))
		.unwrap();
	app
}

pub(super) fn waiting(started: Cancellation, settled: Cancellation) -> Application {
	let action = Action::new(
		"wait",
		"Wait for cancellation",
		move |_: Input, context: InvocationContext<()>| {
			let started = started.clone();
			let settled = settled.clone();
			async move {
				started.cancel();
				context.cancellation().cancelled().await;
				futures_lite::future::yield_now().await;
				settled.cancel();
				Ok::<_, ()>(json!({}))
			}
		},
	)
	.unwrap();
	let mut app = catalog();
	app.register(Entry::new(action).mcp(StructuredBinding::serde()))
		.unwrap();
	app
}

pub(super) struct PausedFrame<'a> {
	bytes: &'a mut Vec<u8>,
	boundary: usize,
	paused: Box<Cancellation>,
	resume: Option<Pin<Box<dyn Future<Output = ()>>>>,
}

impl<'a> PausedFrame<'a> {
	pub(super) fn new(bytes: &'a mut Vec<u8>, resume: Cancellation) -> Self {
		Self::after_bytes(bytes, 1, Cancellation::new(), resume)
	}

	pub(super) fn after_bytes(
		bytes: &'a mut Vec<u8>,
		boundary: usize,
		paused: Cancellation,
		resume: Cancellation,
	) -> Self {
		Self {
			bytes,
			boundary,
			paused: Box::new(paused),
			resume: Some(Box::pin(async move { resume.cancelled().await })),
		}
	}
}

impl AsyncWrite for PausedFrame<'_> {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		let remaining = self.boundary.saturating_sub(self.bytes.len());
		if remaining > 0 {
			let n = remaining.min(buf.len());
			self.bytes.extend_from_slice(&buf[..n]);
			return Poll::Ready(Ok(n));
		}
		self.paused.cancel();
		if let Some(resume) = &mut self.resume {
			std::task::ready!(resume.as_mut().poll(cx));
			self.resume = None;
		}

		self.bytes.extend_from_slice(buf);
		Poll::Ready(Ok(buf.len()))
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.poll_flush(cx)
	}
}
