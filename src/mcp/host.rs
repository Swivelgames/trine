//! Multiplex input, tool settlement and a single bounded output writer.

use super::{
	FRAME_BYTES,
	calls::Calls,
	delivery::{self, Output},
	protocol,
	session::{Dispatch, Session},
};
use crate::{
	Application, Cancellation,
	jsonrpc::framing::{Frame, Frames},
};
use futures_lite::{
	future,
	io::{AsyncRead, AsyncWrite},
};
use serde_json::Value;
use std::io;

enum Closed {
	Shutdown,
	Output(io::Result<()>),
	Input(io::Result<()>),
}

enum Event {
	Frame(io::Result<Frame>),
	Settled(Option<Value>),
}

/// Serve one client. EOF or loss of output requests cooperative cancellation
/// and returns only after all admitted actions settle.
pub async fn serve(
	app: &Application,
	reader: impl AsyncRead + Unpin,
	writer: impl AsyncWrite + Unpin,
	shutdown: Cancellation,
) -> io::Result<()> {
	let session = Session::new(app)?;
	let (outgoing, incoming) = Output::new();
	let mut calls = Calls::default();

	let mut delivery = std::pin::pin!(delivery::write(writer, incoming));
	let receive = read_requests(app, session, reader, &outgoing, &mut calls);
	let stop = async {
		shutdown.cancelled().await;
		Closed::Shutdown
	};
	let input = async { Closed::Input(receive.await) };
	let output = async { Closed::Output(delivery.as_mut().await) };
	let closed = future::or(stop, future::or(output, input)).await;

	calls.withdraw_all();
	outgoing.close();
	match closed {
		Closed::Output(result) | Closed::Input(result @ Err(_)) => {
			calls.settle().await;
			result
		}
		Closed::Input(Ok(())) => {
			// stdin EOF still allows the client to read replies already accepted.
			let drain = future::or(delivery.as_mut(), async {
				shutdown.cancelled().await;
				Err(io::Error::new(
					io::ErrorKind::Interrupted,
					"MCP output drain interrupted",
				))
			});
			let (_, written) = future::zip(calls.settle(), drain).await;
			written
		}
		Closed::Shutdown => {
			calls.settle().await;
			future::poll_once(delivery).await.unwrap_or_else(|| {
				Err(io::Error::new(
					io::ErrorKind::Interrupted,
					"MCP output drain interrupted",
				))
			})
		}
	}
}

async fn read_requests(
	app: &Application,
	mut session: Session<'_>,
	reader: impl AsyncRead + Unpin,
	outgoing: &Output,
	calls: &mut Calls,
) -> io::Result<()> {
	let mut frames = Frames::new(reader);
	loop {
		let event = future::or(async { Event::Settled(calls.next().await) }, async {
			Event::Frame(frames.next(FRAME_BYTES).await)
		})
		.await;
		let reply = match event {
			Event::Settled(reply) => reply,
			Event::Frame(frame) => match frame? {
				Frame::Eof => return Ok(()),
				Frame::Oversized => Some(protocol::error(
					Value::Null,
					-32001,
					"Input frame exceeds the limit",
				)),
				Frame::Data(bytes) => dispatch(app, &mut session, calls, outgoing, &bytes),
			},
		};
		if let Some(reply) = reply {
			outgoing.reply(&reply)?;
		}

		// A ready input stream must still give the writer and shutdown a turn.
		future::yield_now().await;
	}
}

fn dispatch(
	app: &Application,
	session: &mut Session<'_>,
	calls: &mut Calls,
	output: &Output,
	bytes: &[u8],
) -> Option<Value> {
	let request = match protocol::decode(bytes, |id| calls.contains(id)) {
		Ok(request) => request,
		Err(reply) => return Some(reply),
	};
	match session.route(request) {
		Dispatch::Ignore => None,
		Dispatch::Reply(reply) => Some(reply),
		Dispatch::Cancel(id) => {
			calls.cancel(&id);
			None
		}
		Dispatch::Call {
			id,
			name,
			arguments,
			token,
		} => calls.start(app, id, &name, arguments, token, output),
	}
}
