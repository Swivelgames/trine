//! Structured input and view assembly for harness and live adapters.

use crate::application::{
	AsyncStructuredReport, InvocationError, PreparedStructured, StructuredPrepare,
};
use crate::{
	Action, Cancellation,
	diagnostics::{self, Diagnostic, DiagnosticRenderer},
	error::{Failure, FailureKind},
	execution::{PreparedInvocation, PresentationFailure, PresentationState},
	registry::DispatchReport,
	structured::{GuardedView, LiveSink, StructuredBinding, StructuredContext},
};
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub(super) fn structured_prepare<I, O, E, P>(
	action: Action<I, O, E, P>,
	binding: StructuredBinding<O, E, P>,
) -> Arc<StructuredPrepare>
where
	I: DeserializeOwned + Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
{
	let factory = binding.factory;
	let capacity = binding.capacity;
	let prepare = Box::new(
		move |live: Option<Arc<dyn LiveSink>>,
		      value,
		      observe,
		      cancellation: Cancellation,
		      renderer: DiagnosticRenderer| {
			let input = serde_json::from_value(value)
				.map_err(|e| InvocationError::Input(Failure::new(FailureKind::InvalidInput, e)))?;

			let capacity = capacity.filter(|_| observe);
			let is_live = live.is_some();
			let context = match live {
				Some(sink) => StructuredContext::with_live(capacity.is_some(), sink),
				None => StructuredContext::new(capacity.is_some()),
			};

			let close = context.close_on_drop();
			let view = factory(context.clone()).map_err(InvocationError::Setup)?;
			let view = GuardedView {
				view,
				context: context.clone(),
			};

			let invocation = PreparedInvocation::new(
				action.clone(),
				input,
				view,
				capacity,
				cancellation.clone(),
			);

			let future = async move {
				let _close = close;
				let mut report = invocation.run().await;
				if let Err(error) = context.close(false)
					&& !report.presentation.is_err()
				{
					report.presentation =
						PresentationState::Failed(PresentationFailure::Output(error));
				}

				let mut execution = DispatchReport::from_report(report);
				if !is_live {
					deliver_structured_diagnostic(&mut execution, &renderer, &context);
				}

				AsyncStructuredReport {
					execution,
					transcript: context.transcript(),
				}
			};

			Ok(PreparedStructured {
				cancellation,
				future: Box::pin(future),
			})
		},
	);

	Arc::new(StructuredPrepare { prepare })
}

fn deliver_structured_diagnostic(
	report: &mut DispatchReport,
	renderer: &DiagnosticRenderer,
	context: &StructuredContext,
) {
	let Some(diagnostic) = Diagnostic::from_report(report) else {
		return;
	};

	report.diagnostics = diagnostics::deliver(
		&diagnostic,
		renderer,
		|text| {
			if diagnostic.presentation_failure.is_some() {
				context.fallback_diagnostic(text)
			} else {
				context.framework_diagnostic(text)
			}
		},
		|text| context.fallback_diagnostic(text),
	);
}
