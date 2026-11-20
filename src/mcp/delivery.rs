//! Bounded progress and reply capacity feeding one FIFO writer.

use super::{FRAME_BYTES, QUEUED_FRAMES, protocol};
use crate::{Cancellation, jsonrpc::framing};
use futures_lite::{
	future,
	io::{AsyncWrite, AsyncWriteExt},
};
use serde_json::Value;
use smol::lock::{Semaphore, SemaphoreGuardArc};
use std::{io, sync::Arc};

struct Frame {
	bytes: Vec<u8>,
	withdrawn: Option<Cancellation>,
	slot: SemaphoreGuardArc,
}

pub(super) struct WriterQueue(async_channel::Receiver<Frame>);

impl Drop for WriterQueue {
	fn drop(&mut self) {
		// Retained presentation contexts can still own senders after this writer exits.
		self.0.close();
		while self.0.try_recv().is_ok() {}
	}
}

#[derive(Clone)]
pub(super) struct Output {
	queue: async_channel::Sender<Frame>,
	progress_slots: Arc<Semaphore>,
	reply_slots: Arc<Semaphore>,
}

impl Output {
	pub fn new() -> (Self, WriterQueue) {
		// Each queued frame owns a slot of its kind. A burst of progress can
		// never occupy the slots needed to settle calls or answer control traffic.
		let (queue, incoming) = async_channel::bounded(2 * QUEUED_FRAMES);
		let output = Self {
			queue,
			progress_slots: Arc::new(Semaphore::new(QUEUED_FRAMES)),
			reply_slots: Arc::new(Semaphore::new(QUEUED_FRAMES)),
		};
		(output, WriterQueue(incoming))
	}

	pub fn reply(&self, reply: &Value) -> io::Result<()> {
		let slot = self
			.reply_slots
			.try_acquire_arc()
			.ok_or_else(|| io::Error::other("MCP output capacity exhausted"))?;
		self.enqueue(encode_reply(reply)?, None, slot)
	}

	pub async fn progress_slot(&self) -> SemaphoreGuardArc {
		self.progress_slots.acquire_arc().await
	}

	pub fn progress(
		&self,
		bytes: Vec<u8>,
		withdrawn: Cancellation,
		slot: SemaphoreGuardArc,
	) -> io::Result<()> {
		self.enqueue(bytes, Some(withdrawn), slot)
	}

	fn enqueue(
		&self,
		bytes: Vec<u8>,
		withdrawn: Option<Cancellation>,
		slot: SemaphoreGuardArc,
	) -> io::Result<()> {
		self.queue
			.try_send(Frame {
				bytes,
				withdrawn,
				slot,
			})
			.map_err(|_| io::Error::other("MCP output closed"))
	}

	pub fn close(&self) {
		self.queue.close();
	}
}

pub(super) async fn write(
	mut writer: impl AsyncWrite + Unpin,
	queue: WriterQueue,
) -> io::Result<()> {
	while let Ok(frame) = queue.0.recv().await {
		drop(frame.slot);

		let mut bytes = frame.bytes.as_slice();
		if let Some(withdrawn) = &frame.withdrawn {
			// A pending write has accepted no bytes and can still be withdrawn.
			// After the first byte, the rest belongs to the same JSON line.
			let stop = async {
				withdrawn.cancelled().await;
				Ok(None)
			};
			let first = async { writer.write(bytes).await.map(Some) };
			match future::or(stop, first).await? {
				None => continue,
				Some(0) => return Err(io::ErrorKind::WriteZero.into()),
				Some(n) => bytes = &bytes[n..],
			}
		}
		writer.write_all(bytes).await?;
		writer.flush().await?;
	}
	Ok(())
}

fn encode_reply(reply: &Value) -> io::Result<Vec<u8>> {
	if let Ok(bytes) = framing::encode(reply, FRAME_BYTES) {
		return Ok(bytes);
	}
	let fallback = protocol::error(
		reply["id"].clone(),
		-32603,
		"MCP response exceeds the frame limit",
	);
	// Even an ID occupying almost the entire input frame needs room for
	// the fixed-size error envelope when we echo it back.
	framing::encode(&fallback, FRAME_BYTES + 128).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		jsonrpc::Id,
		mcp::progress::Progress,
		structured::{Destination, StructuredContext},
	};
	use futures_lite::io::Cursor;
	use std::{
		pin::Pin,
		task::{Context, Poll},
	};

	struct Stalled;

	impl AsyncWrite for Stalled {
		fn poll_write(
			self: Pin<&mut Self>,
			_: &mut Context<'_>,
			_: &[u8],
		) -> Poll<io::Result<usize>> {
			Poll::Pending
		}

		fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
			Poll::Pending
		}

		fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
			Poll::Pending
		}
	}

	#[test]
	fn failed_or_interrupted_writer_releases_queued_buffers_with_a_retained_view() {
		future::block_on(async {
			for interrupt in [false, true] {
				let (output, queue) = Output::new();
				let sink = Progress::new(
					Id::String("retained".into()),
					output.clone(),
					Cancellation::new(),
				);
				let view = StructuredContext::new(true, Destination::Progress(Arc::new(sink)));
				for _ in 0..3 {
					view.progress("update".into()).await.unwrap();
				}
				output.close();
				if interrupt {
					let mut drain = Box::pin(write(Stalled, queue));
					assert!(future::poll_once(&mut drain).await.is_none());
					assert_eq!(output.queue.len(), 2);
					drop(drain);
				} else {
					let no_space = Cursor::new(&mut [][..]);
					assert_eq!(
						write(no_space, queue).await.unwrap_err().kind(),
						io::ErrorKind::WriteZero
					);
				}
				assert!(output.queue.is_empty());
				drop(view);
			}
		});
	}
}
