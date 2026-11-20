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
