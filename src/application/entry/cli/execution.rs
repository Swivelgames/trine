//! Settle CLI invocations before delivering terminal results and fallback diagnostics.
use crate::{
	Cancellation,
	application::{
		CliCompletion, CliViewContext, Output, PreparedCliExecution,
		json::{JsonProjection, JsonResponse},
	},
	cli::Cli,
	diagnostics::{self, Diagnostic, DiagnosticRenderer},
	error::{Failure, FailureKind},
	registry::DispatchReport,
	view::{self, ErasedView},
};
use std::{
	num::NonZeroUsize,
	sync::{Arc, Mutex},
};

/// Construct the view before handing the host an executable invocation.
pub(super) fn prepare_human<I, O, E, P>(
	cli: Arc<Cli<I, O, E, P>>,
	input: I,
	view: ErasedView<O, E, P, CliCompletion>,
	stderr: Output,
	cancellation: Cancellation,
	renderer: DiagnosticRenderer,
	capacity: Option<NonZeroUsize>,
) -> PreparedCliExecution
where
	I: Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
{
	PreparedCliExecution {
		cancellation: cancellation.clone(),
		run: Box::new(move || {
			let report = view::run(&cli.action, input, view, capacity, cancellation);
			let mut report = DispatchReport::from_report(report);
			deliver_cli_diagnostic(&mut report, &renderer, &stderr);
			report
		}),
	}
}

pub(super) fn prepare_json<I, O, E, P>(
	cli: Arc<Cli<I, O, E, P>>,
	input: I,
	projection: JsonProjection<O, E>,
	context: CliViewContext,
	cancellation: Cancellation,
	renderer: DiagnosticRenderer,
) -> PreparedCliExecution
where
	I: Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
{
	PreparedCliExecution {
		cancellation: cancellation.clone(),
		run: Box::new(move || {
			let result = Arc::new(Mutex::new(None));
			let view = projection.capture(result.clone());
			let mut report = view::run(&cli.action, input, view, None, cancellation);
			let result = result.lock().expect("JSON result lock").take();
			let response = JsonResponse::from_execution(&report, result);

			if let Err(error) = response.write(&context.stdout) {
				// Do not attempt a second document after an output write may have begun.
				report.presentation = crate::execution::PresentationState::Failed(
					crate::execution::PresentationFailure::Output(error),
				);
				let mut report = DispatchReport::from_report(report);
				deliver_cli_diagnostic(&mut report, &renderer, &context.stderr);
				return report;
			}
			DispatchReport::from_report(report)
		}),
	}
}

fn deliver_cli_diagnostic(
	report: &mut DispatchReport,
	renderer: &DiagnosticRenderer,
	stderr: &Output,
) {
	let Some(diagnostic) = Diagnostic::from_report(report) else {
		return;
	};

	let write = |text: &str| {
		stderr
			.write_all(format!("{text}\n").as_bytes())
			.map_err(|error| Failure::new(FailureKind::OutputFailed, error))
	};

	report.diagnostics = diagnostics::deliver(&diagnostic, renderer, write, write);
}
