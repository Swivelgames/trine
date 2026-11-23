//! Typed structured view factories and observation configuration.

use super::{StructuredContext, serde_view::SerdeView};
use crate::{
	error::{Failure, FailureKind},
	view::{ErasedView, View},
};
use serde::Serialize;
use serde_json::Value;
use std::{num::NonZeroUsize, sync::Arc};

type Factory<O, E, P> =
	dyn Fn(StructuredContext) -> Result<ErasedView<O, E, P>, Failure> + Send + Sync;

/// A typed factory plus explicit observation configuration.
pub struct StructuredBinding<O, E, P> {
	pub(crate) factory: Arc<Factory<O, E, P>>,
	pub(crate) capacity: Option<NonZeroUsize>,
}

impl<O: Sync + 'static, E: Sync + 'static, P: Send + 'static> StructuredBinding<O, E, P> {
	/// Construct each invocation's view with a factory that receives its own output sink.
	/// Progress is initially disabled. Use a custom binding when the public
	/// response needs a different shape from the handler's Rust value. Submit
	/// exactly one terminal payload during [`View::finish`].
	///
	/// ```
	/// use trine::{Action, Application, Cancellation, Entry, InvocationContext};
	/// use trine::error::Failure;
	/// use trine::structured::{Payload, StructuredBinding, StructuredContext};
	/// use trine::view::View;
	/// use serde_json::json;
	///
	/// struct CountView(StructuredContext);
	///
	/// impl View<usize, String, ()> for CountView {
	/// 	type Error = Failure;
	/// 	type Completion = ();
	///
	/// 	async fn finish(&mut self, count: &Result<usize, String>) -> Result<(), Failure> {
	/// 		let payload = match count {
	/// 			Ok(n) => Payload::Success(json!({"files": n})),
	/// 			Err(reason) => Payload::DomainFailure(json!({"reason": reason})),
	/// 		};
	/// 		self.0.terminal(payload).await
	/// 	}
	/// }
	///
	/// let count = Action::new("count", "Count selected files", |files: Vec<String>, _: InvocationContext<()>| async move {
	/// 	Ok::<_, String>(files.len())
	/// })?;
	/// let mut app = Application::new("demo", "1")?;
	/// app.register(Entry::new(count).rpc(StructuredBinding::custom(|ctx| Ok(CountView(ctx)))))?;
	/// let call = app.invoke_rpc(&["count"], json!(["notes.txt"]), false, Cancellation::new())?;
	/// assert_eq!(call.transcript.terminal, Some(Payload::Success(json!({"files": 1}))));
	/// # Ok::<(), Box<dyn std::error::Error>>(())
	/// ```
	#[allow(
		clippy::tabs_in_doc_comments,
		reason = "Use crate indentation in the Rust example"
	)]
	pub fn custom<V, F>(factory: F) -> Self
	where
		V: View<O, E, P, Completion = ()> + 'static,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
		F: Fn(StructuredContext) -> Result<V, Failure> + Send + Sync + 'static,
	{
		Self {
			factory: Arc::new(move |context| factory(context).map(ErasedView::new)),
			capacity: None,
		}
	}

	/// Custom progress presentation is opt-in, and the invocation must also request it.
	pub fn observe(mut self, capacity: NonZeroUsize) -> Self {
		self.capacity = Some(capacity);
		self
	}
}

impl<O: Serialize + Sync + 'static, E: Serialize + Sync + 'static, P: Send + 'static>
	StructuredBinding<O, E, P>
{
	/// Serialize domain successes and errors as terminal payloads. Progress is disabled.
	pub fn serde() -> Self {
		Self::custom(|context| Ok(SerdeView::<O, E, P>::new(context, None)))
	}

	/// Serialize terminal results and map observed events to JSON with a bounded progress buffer.
	pub fn serde_with_progress(
		mapper: impl Fn(P) -> Result<Value, Failure> + Send + Sync + 'static,
		capacity: NonZeroUsize,
	) -> Self {
		let mapper = Arc::new(mapper);
		Self::custom(move |context| Ok(SerdeView::<O, E, P>::new(context, Some(mapper.clone()))))
			.observe(capacity)
	}
}

impl<O, E, P> StructuredBinding<O, E, P>
where
	O: Serialize + Sync + 'static,
	E: Serialize + Sync + 'static,
	P: Serialize + Send + 'static,
{
	/// Serialize progress with Serde when the invocation opts into observation.
	/// Use `serde_with_progress` for a custom progress representation.
	pub fn serde_progress(capacity: NonZeroUsize) -> Self {
		Self::serde_with_progress(
			|event| {
				serde_json::to_value(event)
					.map_err(|error| Failure::new(FailureKind::EncodingFailed, error))
			},
			capacity,
		)
	}
}
