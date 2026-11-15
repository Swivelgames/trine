//! A bounded, invocation-local decision exchange. No response is an explicit
//! unsupported-interaction result, never implicit confirmation.
use super::{Cancellation, EmitError};
use async_channel::{Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
	pub code: String,
	pub message: String,
}

#[derive(Debug)]
pub(crate) struct Request {
	pub confirmation: Confirmation,
	pub response: Sender<Option<bool>>,
}

pub(crate) async fn ask(
	requests: &Sender<Request>,
	confirmation: Confirmation,
	cancellation: &Cancellation,
) -> Result<Option<bool>, EmitError> {
	let (response, answer) = async_channel::bounded(1);
	let exchange = async {
		requests
			.send(Request {
				confirmation,
				response,
			})
			.await
			.map_err(|_| EmitError::Closed)?;
		answer.recv().await.map_err(|_| EmitError::Closed)
	};
	futures_lite::future::or(
		async {
			cancellation.cancelled().await;
			Err(EmitError::Cancelled)
		},
		exchange,
	)
	.await
}

pub(crate) type Requests = Receiver<Request>;
