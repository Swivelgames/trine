//! A cloned presentation context can have a send in flight beyond view settlement.
use super::fixture::*;
use futures_lite::future;
use serde_json::json;
use std::{
	future::Future,
	num::NonZeroUsize,
	pin::Pin,
	sync::{Arc, Mutex},
};
use trine::{
	Action, Cancellation, Entry, InvocationContext,
	error::Failure,
	structured::{Payload, StructuredBinding, StructuredContext},
	view::View,
};

type Pending = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send>>;

/// Keep the first send that waits for space, as a view on another executor could.
fn fill_progress(context: &StructuredContext) -> Pending {
	for _ in 0..100 {
		let retained = context.clone();
		let mut send: Pending = Box::pin(async move { retained.progress(json!("old")).await });
		match future::block_on(future::poll_once(send.as_mut())) {
			Some(result) => result.unwrap(),
			None => return send,
		}
	}
	panic!("The stalled writer accepted every update");
}

struct Terminal(StructuredContext);

impl View<(), (), ()> for Terminal {
	type Completion = ();
	type Error = Failure;

	async fn progress(&mut self, _: ()) -> Result<(), Failure> {
		Ok(())
	}

	async fn finish(&mut self, _: &Result<(), ()>) -> Result<(), Failure> {
		self.0.terminal(Payload::Success(json!({}))).await
	}
}

#[test]
fn held_progress_cannot_escape_completed_or_failed_preparation_scopes() {
	for reject_setup in [false, true] {
		let mut app = catalog();
		let pending = Arc::new(Mutex::new(None));
		let held = pending.clone();
		let binding = StructuredBinding::custom(move |context| {
			*held.lock().unwrap() = Some(fill_progress(&context));
			if reject_setup {
				return Err("setup failed".into());
			}
			Ok(Terminal(context))
		})
		.observe(NonZeroUsize::MIN);
		let original = Action::new(
			"original",
			"Produce updates",
			|_: Input, _: InvocationContext<()>| async { Ok::<_, ()>(()) },
		)
		.unwrap();
		app.register(Entry::new(original).mcp(binding)).unwrap();

		let resume = Cancellation::new();
		let release = resume.clone();
		let reuse = Action::new(
			"reuse",
			"Reuse the token",
			move |_: Input, _: InvocationContext<()>| {
				let pending = pending.lock().unwrap().take().unwrap();
				let release = release.clone();
				async move {
					// The original send must resolve while output is still blocked.
					pending.await.unwrap();
					release.cancel();
					Ok::<_, ()>(json!({}))
				}
			},
		)
		.unwrap();
		app.register(Entry::new(reuse).mcp(StructuredBinding::serde()))
			.unwrap();

		let mut requests = ready();
		requests.extend([
			subscribed(2, "original", json!("same")),
			subscribed(3, "reuse", json!("same")),
		]);
		let mut bytes = Vec::new();
		let writer = PausedFrame::new(&mut bytes, resume);
		serve(&app, lines(&requests), writer, Cancellation::new()).unwrap();

		let frames = replies(&bytes);
		let terminal = frames.iter().position(|frame| frame["id"] == 2).unwrap();
		if reject_setup {
			assert_eq!(terminal, 1, "Rejected view left progress in the writer");
			assert_eq!(frames[terminal]["error"]["code"], -32603);
		} else {
			assert!(terminal > 1);
			assert!(
				frames[1..terminal]
					.iter()
					.all(|frame| frame["params"]["message"] == "old")
			);
			assert_eq!(frames[terminal]["result"]["structuredContent"], json!({}));
		}

		let reused = &frames[terminal + 1..];
		assert_eq!(reused.len(), 1);
		assert_eq!(reused[0]["id"], 3);
		assert_eq!(reused[0]["result"]["structuredContent"], json!({}));
	}
}
