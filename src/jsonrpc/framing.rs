//! Bounded newline-delimited input and output, independent of request semantics.

use crate::error::{Failure, FailureKind};
use futures_lite::io::{AsyncRead, AsyncReadExt};
use serde_json::Value;
use std::io::{self, Write};

pub(crate) enum Frame {
	Data(Vec<u8>),
	Oversized,
	Eof,
}

pub(crate) struct Frames<R> {
	reader: futures_lite::io::BufReader<R>,
	bytes: Vec<u8>,
	oversized: bool,
	pending_cr: bool,
	bytes_since_yield: usize,
}

impl<R: AsyncRead + Unpin> Frames<R> {
	pub fn new(reader: R) -> Self {
		Self {
			reader: futures_lite::io::BufReader::new(reader),
			bytes: Vec::new(),
			oversized: false,
			pending_cr: false,
			bytes_since_yield: 0,
		}
	}

	pub async fn next(&mut self, limit: usize) -> io::Result<Frame> {
		loop {
			// Even an always-ready stream (including oversized discard) must let
			// control and owned invocations run. Parser state survives cancellation.
			if self.bytes_since_yield == 8192 {
				self.bytes_since_yield = 0;
				futures_lite::future::yield_now().await;
			}

			let mut byte = [0];
			let eof = self.reader.read(&mut byte).await? == 0;
			self.bytes_since_yield += usize::from(!eof);

			if eof || byte[0] == b'\n' {
				if eof && self.pending_cr {
					self.push(b'\r', limit);
				}
				self.pending_cr = false;

				if self.oversized {
					self.oversized = false;
					self.bytes.clear();
					return Ok(Frame::Oversized);
				}
				if !self.bytes.is_empty() {
					return Ok(Frame::Data(std::mem::take(&mut self.bytes)));
				}
				if eof {
					return Ok(Frame::Eof);
				}
				continue;
			}

			if self.pending_cr {
				self.push(b'\r', limit);
			}
			self.pending_cr = byte[0] == b'\r';
			if !self.pending_cr {
				self.push(byte[0], limit);
			}
		}
	}

	fn push(&mut self, byte: u8, limit: usize) {
		if self.oversized {
			return;
		}

		if self.bytes.len() == limit {
			self.oversized = true;
			self.bytes.clear();
		} else {
			self.bytes.push(byte);
		}
	}
}

struct Limited {
	bytes: Vec<u8>,
	limit: usize,
	exceeded: bool,
}

impl Write for Limited {
	fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
		if bytes.len() > self.limit - self.bytes.len() {
			self.exceeded = true;
			return Err(io::Error::other("output_too_large"));
		}

		self.bytes.extend_from_slice(bytes);
		Ok(bytes.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

pub(crate) fn encode(value: &Value, limit: usize) -> Result<Vec<u8>, Failure> {
	let mut writer = Limited {
		bytes: Vec::new(),
		limit,
		exceeded: false,
	};

	if let Err(error) = serde_json::to_writer(&mut writer, value) {
		let kind = if writer.exceeded {
			FailureKind::OutputTooLarge
		} else {
			FailureKind::EncodingFailed
		};
		return Err(Failure::new(kind, error));
	}

	writer.bytes.push(b'\n');
	Ok(writer.bytes)
}
