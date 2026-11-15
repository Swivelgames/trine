//! Complete accepted progress delivery before asking the view for a decision.
use super::View;
use crate::{Cancellation, invocation::Request};

pub(super) async fn deliver<O, E, P, V: View<O, E, P>>(
	view: &mut V,
	events: Option<&async_channel::Receiver<P>>,
	request: Request,
	cancellation: &Cancellation,
) -> Result<(), V::Error> {
	// emit().await completes before confirm() submits its request. All of those
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

	let answer = futures_lite::future::or(
		view.confirm(request.confirmation, cancellation.clone()),
		async {
			cancellation.cancelled().await;
			Ok(None)
		},
	)
	.await?;
	let _ = request.response.try_send(answer);
	Ok(())
}
