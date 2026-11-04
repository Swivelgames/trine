//! Typed structured view factories and observation configuration.

use super::{StructuredContext, serde_view::SerdeView};
use crate::{
	error::Failure,
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
	pub fn serde() -> Self {
		Self::custom(|context| Ok(SerdeView::<O, E, P>::new(context, None)))
	}

	pub fn serde_with_progress(
		mapper: impl Fn(P) -> Result<Value, Failure> + Send + Sync + 'static,
		capacity: NonZeroUsize,
	) -> Self {
		let mapper = Arc::new(mapper);
		Self::custom(move |context| Ok(SerdeView::<O, E, P>::new(context, Some(mapper.clone()))))
			.observe(capacity)
	}
}
