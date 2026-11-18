//! Complete accepted progress delivery before asking the view for a decision.
use super::View;
use crate::{Cancellation, invocation::Request};

pub(super) async fn deliver<O, E, P, V: View<O, E, P>>(
	view: &mut V,
	events: Option<&async_channel::Receiver<P>>,
	request: Request,
	cancellation: &Cancellation,
) -> Result<(), V::Error> {
	// emit().await completes before the action submits its decision request. All of those
	// earlier events are already queued or rendered. Snapshot the queue length so
	// unrelated concurrent emitters cannot keep extending the prompt's wait.
	if let Some(events) = events {
		let accepted = events.len();
		for _ in 0..accepted {
			let Ok(event) = events.try_recv() else {
				break;
			};
			view.progress(event).await?;
		}
	}

	match request {
		Request::Confirm(prompt, response) => {
			deliver_answer(
				view.confirm(prompt, cancellation.clone()),
				response,
				cancellation,
			)
			.await
		}
		Request::Select(prompt, response) => {
			deliver_answer(
				view.select(prompt, cancellation.clone()),
				response,
				cancellation,
			)
			.await
		}
	}
}

async fn deliver_answer<T, E>(
	prompt: impl Future<Output = Result<Option<T>, E>>,
	response: async_channel::Sender<Option<T>>,
	cancellation: &Cancellation,
) -> Result<(), E> {
	let answer = futures_lite::future::or(prompt, async {
		cancellation.cancelled().await;
		Ok(None)
	})
	.await?;
	let _ = response.try_send(answer);
	Ok(())
}
