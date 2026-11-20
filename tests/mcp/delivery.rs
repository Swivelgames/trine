use super::fixture::*;
use futures_lite::{
	future,
	io::{AsyncRead, AsyncReadExt, AsyncWrite, Cursor},
};
use serde_json::json;
use std::{
	future::Future,
	io,
	pin::Pin,
	task::{Context, Poll},
};
use trine::{Action, Cancellation, Entry, InvocationContext, mcp, structured::StructuredBinding};

struct BrokenAfterStart(Cancellation);

impl AsyncWrite for BrokenAfterStart {
	fn poll_write(
		self: Pin<&mut Self>,
		_: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		if self.0.is_cancelled() {
			return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
		}
		Poll::Ready(Ok(buf.len()))
	}

	fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
		Poll::Ready(Ok(()))
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.poll_flush(cx)
	}
}

#[test]
fn broken_output_cancels_and_joins_active_work() {
	let started = Cancellation::new();
	let settled = Cancellation::new();
	let app = waiting(started.clone(), settled.clone());
	let mut requests = ready();
	requests.extend([call(2, "wait"), request(3, "ping", json!({}))]);

	let error = serve(
		&app,
		lines(&requests),
		Box::pin(BrokenAfterStart(started)),
		Cancellation::new(),
	)
	.unwrap_err();
	assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
	assert!(settled.is_cancelled());
}

#[test]
fn half_closed_input_settles_calls_and_flushes_when_the_reader_resumes() {
	let settled = Cancellation::new();
	let app = waiting(Cancellation::new(), settled.clone());
	let mut requests = ready();
	requests.push(call(2, "wait"));

	let resume = Cancellation::new();
	let mut output = Vec::new();
	let writer = PausedFrame::new(&mut output, resume.clone());
	let host = mcp::serve(
		&app,
		Cursor::new(lines(&requests)),
		writer,
		Cancellation::new(),
	);
	let resume_later = async {
		settled.cancelled().await;
		future::yield_now().await;
		resume.cancel();
	};

	let (result, ()) = run(future::zip(host, resume_later));
	result.unwrap();
	assert_eq!(replies(&output)[0]["id"], 1);
}

struct OpenInput;

impl AsyncRead for OpenInput {
	fn poll_read(
		self: Pin<&mut Self>,
		_: &mut Context<'_>,
		_: &mut [u8],
	) -> Poll<io::Result<usize>> {
		Poll::Pending
	}
}

#[test]
fn host_shutdown_joins_calls_with_input_open_and_reports_abandoned_output() {
	let shutdown = Cancellation::new();
	let settled = Cancellation::new();
	let app = waiting(shutdown.clone(), settled.clone());
	let mut requests = ready();
	requests.push(call(2, "wait"));
	let reader = Cursor::new(lines(&requests)).chain(OpenInput);

	let mut output = Vec::new();
	let writer = PausedFrame::new(&mut output, Cancellation::new());
	let error = run(mcp::serve(&app, reader, writer, shutdown)).unwrap_err();
	assert_eq!(error.kind(), io::ErrorKind::Interrupted);
	assert_eq!(output, b"{");
	assert!(settled.is_cancelled());
}

#[test]
fn host_shutdown_can_interrupt_a_stalled_half_close() {
	let settled = Cancellation::new();
	let app = waiting(Cancellation::new(), settled.clone());
	let mut requests = ready();
	requests.push(call(2, "wait"));

	let mut output = Vec::new();
	let writer = PausedFrame::new(&mut output, Cancellation::new());
	let shutdown = Cancellation::new();
	let host = mcp::serve(
		&app,
		Cursor::new(lines(&requests)),
		writer,
		shutdown.clone(),
	);
	let interrupt_drain = async {
		settled.cancelled().await;
		shutdown.cancel();
	};
	let (result, ()) = run(future::zip(host, interrupt_drain));
	assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
	assert_eq!(output, b"{");
}

#[test]
fn stalled_output_has_a_bounded_queue_and_settles_admitted_calls_on_overflow() {
	let settled = Cancellation::new();
	let app = waiting(Cancellation::new(), settled.clone());
	let mut requests = ready();
	requests.push(call(2, "wait"));
	requests.extend((3..30).map(|id| request(id, "ping", json!({}))));

	let mut output = Vec::new();
	let writer = PausedFrame::new(&mut output, Cancellation::new());
	let error = serve(&app, lines(&requests), writer, Cancellation::new()).unwrap_err();
	assert!(error.to_string().contains("capacity"));
	assert!(settled.is_cancelled());
}

/// Release the next input frames when the test reaches its synchronization point.
struct After {
	gate: Option<Pin<Box<dyn Future<Output = ()>>>>,
	input: Cursor<Vec<u8>>,
}

impl After {
	fn new(gate: Cancellation, requests: &[serde_json::Value]) -> Self {
		Self {
			gate: Some(Box::pin(async move { gate.cancelled().await })),
			input: Cursor::new(lines(requests)),
		}
	}
}

impl AsyncRead for After {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &mut [u8],
	) -> Poll<io::Result<usize>> {
		if let Some(gate) = &mut self.gate {
			std::task::ready!(gate.as_mut().poll(cx));
			self.gate = None;
		}
		Pin::new(&mut self.input).poll_read(cx, bytes)
	}
}

#[test]
fn backpressured_calls_accept_control_and_discard_queued_notifications_on_withdrawal() {
	for disconnect in [false, true] {
		let full = Cancellation::new();
		let settled = Cancellation::new();
		let release = settled.clone();
		let action = Action::new(
			"burst",
			"Emit until cancelled",
			move |_: Input, context: InvocationContext<u32>| {
				let release = release.clone();
				async move {
					for i in 1..1000 {
						if context.emit(i).await.is_err() {
							assert!(context.cancellation().is_cancelled());
							release.cancel();
							return Ok::<_, ()>(());
						}
					}
					panic!("Stalled output failed to backpressure the handler");
				}
			},
		)
		.unwrap();
		let gate = full.clone();
		let binding = StructuredBinding::serde_with_progress(
			move |i| {
				// All progress slots are occupied by the first sixteen updates.
				if i == 17 {
					gate.cancel();
				}
				Ok(json!(format!("Update {i}")))
			},
			std::num::NonZeroUsize::MIN,
		);
		let mut app = catalog();
		app.register(Entry::new(action).mcp(binding)).unwrap();

		let mut prefix = ready();
		prefix.push(subscribed(2, "burst", json!("withdrawn")));
		let mut control = vec![request(3, "ping", json!({}))];
		if !disconnect {
			control.push(notification(
				"notifications/cancelled",
				json!({"requestId":2}),
			));
		}
		let reader = Cursor::new(lines(&prefix)).chain(After::new(full, &control));
		let mut output = Vec::new();
		let writer = PausedFrame::new(&mut output, settled.clone());
		run(mcp::serve(&app, reader, writer, Cancellation::new())).unwrap();

		assert!(settled.is_cancelled());
		let frames = replies(&output);
		assert_eq!(frames.len(), 2);
		assert_eq!(frames[0]["id"], 1);
		assert_eq!(frames[1]["id"], 3);
		assert_eq!(frames[1]["result"], json!({}));
	}
}

#[test]
fn withdrawal_skips_an_unwritten_frame_and_finishes_a_partial_frame() {
	let handshake_bytes = lines(&exchange(&catalog(), &[initialize()])).len();

	for first_bytes in [0, 1] {
		let resume = Cancellation::new();
		let release = resume.clone();
		let action = Action::new(
			"waiting",
			"Report and wait",
			move |_: Input, context: InvocationContext<String>| {
				let release = release.clone();
				async move {
					context.emit("Started".to_owned()).await.unwrap();
					context.cancellation().cancelled().await;
					release.cancel();
					Ok::<_, ()>(())
				}
			},
		)
		.unwrap();
		let mut app = catalog();
		app.register(Entry::new(action).mcp(StructuredBinding::serde_progress(
			std::num::NonZeroUsize::MIN,
		)))
		.unwrap();

		let mut prefix = ready();
		prefix.push(subscribed(2, "waiting", json!("withdrawn")));
		let paused = Cancellation::new();
		let control = [
			notification("notifications/cancelled", json!({"requestId":2})),
			request(3, "ping", json!({})),
		];
		let reader = Cursor::new(lines(&prefix)).chain(After::new(paused.clone(), &control));
		let mut bytes = Vec::new();
		let writer =
			PausedFrame::after_bytes(&mut bytes, handshake_bytes + first_bytes, paused, resume);
		run(mcp::serve(&app, reader, writer, Cancellation::new())).unwrap();

		let frames = replies(&bytes);
		assert_eq!(frames.len(), 2 + first_bytes);
		assert_eq!(frames[0]["id"], 1);
		if first_bytes > 0 {
			assert_eq!(frames[1]["method"], "notifications/progress");
			assert_eq!(frames[1]["params"]["message"], "Started");
		}
		assert_eq!(frames.last().unwrap()["id"], 3);
	}
}

#[test]
fn sixteen_replies_can_queue_behind_a_stalled_write() {
	let settled = Cancellation::new();
	let app = waiting(Cancellation::new(), settled.clone());
	let mut requests = ready();
	requests.push(call(2, "wait"));
	requests.extend((3..19).map(|id| request(id, "ping", json!({}))));

	let mut bytes = Vec::new();
	let stalled_until_eof = PausedFrame::new(&mut bytes, settled);
	serve(
		&app,
		lines(&requests),
		stalled_until_eof,
		Cancellation::new(),
	)
	.unwrap();
	let frames = replies(&bytes);
	let ids: Vec<_> = frames
		.iter()
		.map(|frame| frame["id"].as_u64().unwrap())
		.collect();
	assert_eq!(ids, std::iter::once(1).chain(3..19).collect::<Vec<_>>());
}

#[test]
fn a_stalled_progress_stream_resumes_in_order_and_finishes() {
	#[derive(serde::Serialize)]
	struct Outcome {
		#[serde(skip)]
		eof: Cancellation,
	}

	impl Drop for Outcome {
		fn drop(&mut self) {
			self.eof.cancel();
		}
	}

	let full = Cancellation::new();
	let resume = full.clone();
	let binding = StructuredBinding::serde_with_progress(
		move |i| {
			if i == 17 {
				resume.cancel();
			}
			Ok(json!(format!("Update {i}")))
		},
		std::num::NonZeroUsize::MIN,
	);

	let presented = Cancellation::new();
	let eof = presented.clone();
	let action = Action::new(
		"burst",
		"Emit forty updates",
		move |_: Input, context: InvocationContext<u32>| {
			let eof = eof.clone();
			async move {
				for i in 1..=40 {
					context.emit(i).await.unwrap();
				}
				Ok::<_, ()>(Outcome { eof })
			}
		},
	)
	.unwrap();
	let mut app = catalog();
	app.register(Entry::new(action).mcp(binding)).unwrap();

	let mut requests = ready();
	requests.push(subscribed(2, "burst", json!("resume")));
	let reader = Cursor::new(lines(&requests)).chain(After::new(presented, &[]));
	let mut bytes = Vec::new();
	let writer = PausedFrame::new(&mut bytes, full);
	run(mcp::serve(&app, reader, writer, Cancellation::new())).unwrap();

	let frames = replies(&bytes);
	assert_eq!(frames.len(), 42);
	for (i, update) in (1..=40).zip(&frames[1..41]) {
		assert_eq!(
			*update,
			notification(
				"notifications/progress",
				json!({"progressToken":"resume","progress":i,"message":format!("Update {i}")})
			)
		);
	}
	assert_eq!(frames[41]["id"], 2);
	assert_eq!(frames[41]["result"]["structuredContent"], json!({}));
}
