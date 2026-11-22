//! Windows process streams backed by cancellable, joined synchronous I/O workers.

use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
	future::Future,
	io::{self, Read, Write},
	os::windows::io::AsRawHandle,
	pin::Pin,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	task::{Context, Poll},
	thread::{self, JoinHandle},
};

type Pending<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

#[link(name = "kernel32")]
unsafe extern "system" {
	fn CancelSynchronousIo(thread: *mut std::ffi::c_void) -> i32;
}

struct Worker {
	stopped: Arc<AtomicBool>,
	thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
	fn drop(&mut self) {
		self.stopped.store(true, Ordering::Release);
		let Some(thread) = self.thread.take() else {
			return;
		};

		// Cancellation can race the next synchronous call. Repeat until joined,
		// rather than treating ERROR_NOT_FOUND as proof that the worker stopped.
		while !thread.is_finished() {
			unsafe {
				CancelSynchronousIo(thread.as_raw_handle());
			}
			thread::yield_now();
		}

		let _ = thread.join();
	}
}

/// Read process stdin on a dedicated worker.
pub struct Reader {
	_worker: Worker,
	receiver: Box<async_channel::Receiver<io::Result<Vec<u8>>>>,
	pending: Option<Pending<Vec<u8>>>,
	bytes: Vec<u8>,
	offset: usize,
}

impl Drop for Reader {
	fn drop(&mut self) {
		self.receiver.close();
	}
}

impl AsyncRead for Reader {
	fn poll_read(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &mut [u8],
	) -> Poll<io::Result<usize>> {
		if bytes.is_empty() {
			return Poll::Ready(Ok(0));
		}

		if self.offset == self.bytes.len() {
			if self.pending.is_none() {
				let receiver = self.receiver.clone();
				self.pending = Some(Box::pin(async move {
					receiver.recv().await.unwrap_or_else(|_| Ok(Vec::new()))
				}));
			}

			let result = match self.pending.as_mut().unwrap().as_mut().poll(cx) {
				Poll::Pending => return Poll::Pending,
				Poll::Ready(result) => result,
			};
			self.pending = None;
			self.bytes = result?;
			self.offset = 0;
		}

		let count = bytes.len().min(self.bytes.len() - self.offset);
		bytes[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
		self.offset += count;
		Poll::Ready(Ok(count))
	}
}

enum Command {
	Write(Vec<u8>),
	Flush,
}

/// Write and flush process stdout on a dedicated worker.
pub struct Writer {
	_worker: Worker,
	sender: async_channel::Sender<Command>,
	replies: Box<async_channel::Receiver<io::Result<()>>>,
	pending: Option<Pending<()>>,
	flushing: bool,
}

impl Drop for Writer {
	fn drop(&mut self) {
		self.sender.close();
		self.replies.close();
	}
}

impl Writer {
	fn submit(&mut self, command: Command) -> io::Result<()> {
		let flushing = matches!(command, Command::Flush);
		self.sender
			.try_send(command)
			.map_err(|_| io::Error::other("stdio worker closed"))?;

		self.flushing = flushing;
		let replies = self.replies.clone();
		self.pending = Some(Box::pin(async move {
			replies
				.recv()
				.await
				.map_err(|_| io::Error::other("stdio worker closed"))?
		}));
		Ok(())
	}

	fn poll_pending(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		let Some(pending) = &mut self.pending else {
			return Poll::Ready(Ok(()));
		};
		let result = std::task::ready!(pending.as_mut().poll(cx));
		self.pending = None;
		self.flushing = false;
		Poll::Ready(result)
	}
}

impl AsyncWrite for Writer {
	fn poll_write(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		bytes: &[u8],
	) -> Poll<io::Result<usize>> {
		std::task::ready!(self.poll_pending(cx))?;
		if bytes.is_empty() {
			return Poll::Ready(Ok(0));
		}

		// Once submitted, the worker owns these bytes. A pending poll must
		// never submit a buffer that its caller can still withdraw or replace.
		self.submit(Command::Write(bytes.to_vec()))?;
		Poll::Ready(Ok(bytes.len()))
	}

	fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		let finishing_flush = self.flushing;
		std::task::ready!(self.poll_pending(cx))?;
		if finishing_flush {
			return Poll::Ready(Ok(()));
		}

		self.submit(Command::Flush)?;
		self.poll_pending(cx)
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.poll_flush(cx)
	}
}

/// Open exclusive adapters for process stdin and stdout. Dropping them joins their workers.
pub fn open() -> io::Result<(Reader, Writer)> {
	let stopped = Arc::new(AtomicBool::new(false));
	let read_stop = stopped.clone();
	let (input_tx, input_rx) = async_channel::bounded(1);
	let thread = thread::Builder::new()
		.name("trine-stdin".into())
		.spawn(move || {
			let mut input = std::io::stdin().lock();
			while !read_stop.load(Ordering::Acquire) {
				let mut bytes = vec![0; 8192];
				let result = input.read(&mut bytes).map(|count| {
					bytes.truncate(count);
					bytes
				});
				let done = result.as_ref().map_or(true, Vec::is_empty);
				if input_tx.send_blocking(result).is_err() || done {
					break;
				}
			}
		})?;

	let reader = Reader {
		_worker: Worker {
			stopped,
			thread: Some(thread),
		},
		receiver: Box::new(input_rx),
		pending: None,
		bytes: Vec::new(),
		offset: 0,
	};

	let stopped = Arc::new(AtomicBool::new(false));
	let write_stop = stopped.clone();
	let (commands, receive) = async_channel::bounded(1);
	let (reply, replies) = async_channel::bounded(1);
	let thread = thread::Builder::new()
		.name("trine-stdout".into())
		.spawn(move || {
			let mut output = std::io::stdout().lock();
			while let Ok(command) = receive.recv_blocking() {
				if write_stop.load(Ordering::Acquire) {
					break;
				}
				let result = match command {
					Command::Write(bytes) => output.write_all(&bytes),
					Command::Flush => output.flush(),
				};
				if reply.send_blocking(result).is_err() {
					break;
				}
			}
		})?;

	Ok((
		reader,
		Writer {
			_worker: Worker {
				stopped,
				thread: Some(thread),
			},
			sender: commands,
			replies: Box::new(replies),
			pending: None,
			flushing: false,
		},
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures_lite::{future, io::AsyncWriteExt};

	#[test]
	fn worker_acceptance_survives_cancelled_write_and_flush_waits() {
		let (sender, commands) = async_channel::bounded(1);
		let (reply, replies) = async_channel::bounded(1);
		let mut writer = Writer {
			_worker: Worker {
				stopped: Arc::default(),
				thread: None,
			},
			sender,
			replies: Box::new(replies),
			pending: None,
			flushing: false,
		};

		future::block_on(async {
			assert_eq!(writer.write(b"first").await.unwrap(), 5);
			assert!(
				matches!(commands.try_recv().unwrap(), Command::Write(bytes) if bytes == b"first")
			);
			assert!(
				future::poll_once(writer.write(b"withdrawn"))
					.await
					.is_none()
			);
			assert!(commands.is_empty());

			reply.try_send(Ok(())).unwrap();
			assert!(future::poll_once(writer.flush()).await.is_none());
			assert!(matches!(commands.try_recv().unwrap(), Command::Flush));
			assert!(
				future::poll_once(writer.write(b"also withdrawn"))
					.await
					.is_none()
			);
			assert!(commands.is_empty());

			reply.try_send(Ok(())).unwrap();
			assert_eq!(writer.write(b"replacement").await.unwrap(), 11);
			assert!(
				matches!(commands.try_recv().unwrap(), Command::Write(bytes) if bytes == b"replacement")
			);
			reply
				.try_send(Err(io::ErrorKind::BrokenPipe.into()))
				.unwrap();
			assert_eq!(
				writer.flush().await.unwrap_err().kind(),
				io::ErrorKind::BrokenPipe
			);

			assert!(future::poll_once(writer.flush()).await.is_none());
			assert!(matches!(commands.try_recv().unwrap(), Command::Flush));
			reply
				.try_send(Err(io::ErrorKind::BrokenPipe.into()))
				.unwrap();
			assert_eq!(
				writer.write(b"after flush").await.unwrap_err().kind(),
				io::ErrorKind::BrokenPipe
			);
			assert!(commands.is_empty());
		});
	}
}
