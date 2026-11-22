//! A bounded, invocation-local decision exchange. No response is an explicit
//! unsupported-interaction result, never implicit confirmation.
use super::{Cancellation, EmitError};
use async_channel::{Receiver, Sender};

/// A yes-or-no decision requested by a handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirmation {
	/// Application-defined identifier for the decision.
	pub code: String,
	/// Question to show the person making the decision.
	pub message: String,
}

/// Select one of the supplied labels. The returned index refers to this order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
	/// Question describing the choice.
	pub message: String,
	/// Ordered labels; a selected index refers to this vector.
	pub options: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum Request {
	Confirm(Confirmation, Sender<Option<bool>>),
	Select(Selection, Sender<Option<usize>>),
}

pub(crate) async fn ask<T>(
	requests: &Sender<Request>,
	make_request: impl FnOnce(Sender<Option<T>>) -> Request,
	cancellation: &Cancellation,
) -> Result<Option<T>, EmitError> {
	let (response, answer) = async_channel::bounded(1);
	let exchange = async {
		requests
			.send(make_request(response))
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
