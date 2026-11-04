//! Exclusive native stdio adapters. Do not concurrently use the process streams
//! through another reader/writer while these adapters are alive.

#[cfg(unix)]
mod platform {
	use futures_lite::io::{AsyncRead, AsyncWrite};
	use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
	use std::{
		fs::File,
		io::{self, Read, Write},
		pin::Pin,
		task::{Context, Poll},
	};

	enum Handle {
		Ready(async_io::Async<File>),
		Regular(File),
	}

	pub struct Stdio {
		handle: Handle,
		original: OFlags,
	}

	impl Stdio {
		fn new(file: File) -> io::Result<Self> {
			let original = fcntl_getfl(&file)?;
			let handle = if file.metadata()?.is_file() {
				Handle::Regular(file)
			} else {
				let restore = file.try_clone()?;
				match async_io::Async::new(file) {
					Ok(handle) => Handle::Ready(handle),
					Err(error) => {
						let _ = fcntl_setfl(&restore, original);
						return Err(error);
					}
				}
			};

			Ok(Self { handle, original })
		}
	}

	impl Drop for Stdio {
		fn drop(&mut self) {
			let file = match &self.handle {
				Handle::Ready(handle) => handle.get_ref(),
				Handle::Regular(file) => file,
			};

			// dup shares status flags with the original process descriptor.
			let _ = fcntl_setfl(file, self.original);
		}
	}

	impl AsyncRead for Stdio {
		fn poll_read(
			mut self: Pin<&mut Self>,
			cx: &mut Context<'_>,
			bytes: &mut [u8],
		) -> Poll<io::Result<usize>> {
			match &mut self.handle {
				Handle::Ready(handle) => Pin::new(handle).poll_read(cx, bytes),
				Handle::Regular(file) => Poll::Ready(file.read(bytes)),
			}
		}
	}

	impl AsyncWrite for Stdio {
		fn poll_write(
			mut self: Pin<&mut Self>,
			cx: &mut Context<'_>,
			bytes: &[u8],
		) -> Poll<io::Result<usize>> {
			match &mut self.handle {
				Handle::Ready(handle) => Pin::new(handle).poll_write(cx, bytes),
				Handle::Regular(file) => Poll::Ready(file.write(bytes)),
			}
		}

		fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
			match &mut self.handle {
				Handle::Ready(handle) => Pin::new(handle).poll_flush(cx),
				Handle::Regular(file) => Poll::Ready(file.flush()),
			}
		}

		fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
			self.poll_flush(cx)
		}
	}

	pub fn open() -> io::Result<(Stdio, Stdio)> {
		let input = File::from(rustix::io::dup(std::io::stdin())?);
		let output = File::from(rustix::io::dup(std::io::stdout())?);

		Ok((Stdio::new(input)?, Stdio::new(output)?))
	}
}

#[cfg(windows)]
mod platform {
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

	pub struct Writer {
		_worker: Worker,
		sender: async_channel::Sender<Command>,
		replies: Box<async_channel::Receiver<io::Result<usize>>>,
		pending: Option<Pending<usize>>,
	}

	impl Drop for Writer {
		fn drop(&mut self) {
			self.sender.close();
			self.replies.close();
		}
	}

	impl Writer {
		fn request(
			&mut self,
			cx: &mut Context<'_>,
			command: impl FnOnce() -> Command,
		) -> Poll<io::Result<usize>> {
			if self.pending.is_none() {
				self.sender
					.try_send(command())
					.map_err(|_| io::Error::other("stdio worker closed"))?;
				let replies = self.replies.clone();
				self.pending = Some(Box::pin(async move {
					replies
						.recv()
						.await
						.map_err(|_| io::Error::other("stdio worker closed"))?
				}));
			}

			match self.pending.as_mut().unwrap().as_mut().poll(cx) {
				Poll::Pending => Poll::Pending,
				Poll::Ready(result) => {
					self.pending = None;
					Poll::Ready(result)
				}
			}
		}
	}

	impl AsyncWrite for Writer {
		fn poll_write(
			mut self: Pin<&mut Self>,
			cx: &mut Context<'_>,
			bytes: &[u8],
		) -> Poll<io::Result<usize>> {
			self.request(cx, || Command::Write(bytes.to_vec()))
		}

		fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
			self.request(cx, || Command::Flush).map(|r| r.map(|_| ()))
		}

		fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
			self.poll_flush(cx)
		}
	}

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
						Command::Write(bytes) => output.write(&bytes),
						Command::Flush => output.flush().map(|_| 0),
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
			},
		))
	}
}

pub use platform::open;
