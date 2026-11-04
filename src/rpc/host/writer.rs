//! Serialize queued output and return delivery receipts to the connection.

use super::Event;
use crate::{Cancellation, rpc::outbox::Outbox};
use futures_lite::{
	future,
	io::{AsyncWrite, AsyncWriteExt},
};
use std::io;

pub(super) async fn write_messages<W: AsyncWrite + Unpin>(
	mut writer: W,
	outbox: Outbox,
	events: async_channel::Sender<Event>,
	stop: Cancellation,
) {
	while let Some(message) = outbox.next().await {
		let write = async {
			writer.write_all(&message.bytes).await?;
			writer.flush().await
		};

		let result = future::race(
			async {
				Some(
					crate::execution::supervise(write)
						.await
						.unwrap_or_else(|_| Err(io::Error::other("RPC writer panicked"))),
				)
			},
			async {
				stop.cancelled().await;
				None
			},
		)
		.await;
		match result {
			None => break,
			Some(Err(error)) => {
				let _ = events.send(Event::WriterFailed(error.to_string())).await;
				return;
			}
			Some(Ok(())) => {
				if let Some(id) = message.terminal {
					let _ = events.send(Event::Written(id)).await;
				}
			}
		}
	}

	let _ = events.send(Event::WriterDone).await;
}
