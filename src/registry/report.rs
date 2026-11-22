//! Type-erased execution and presentation facts retained after dispatch.

use crate::view::Report;
use std::any::Any;

/// Interface-independent classification of view settlement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationStatus {
	/// The view produced a completion value.
	Completed,
	/// Presentation returned an error or panicked.
	Failed,
	/// The handler terminated without a result for the view to present.
	Skipped,
}

trait ReportDetail: Send {
	fn as_any(&self) -> &dyn Any;

	fn failure(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)>;
}

impl<
	O: Send + 'static,
	E: Send + 'static,
	V: std::error::Error + Send + Sync + 'static,
	C: Send + 'static,
> ReportDetail for Report<O, E, V, C>
{
	fn as_any(&self) -> &dyn Any {
		self
	}

	fn failure(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
		match &self.presentation {
			crate::execution::PresentationState::Failed(
				crate::execution::PresentationFailure::Output(error),
			) => Some(error),
			_ => None,
		}
	}
}

/// Settlement facts a host can inspect without knowing the concrete action and view types.
pub struct DispatchReport {
	/// Whether framework diagnostics were delivered or required fallback.
	pub diagnostics: crate::diagnostics::DiagnosticDelivery,
	/// Whether the handler returned an `Ok` domain value.
	pub domain_succeeded: bool,
	/// Whether terminal presentation completed successfully.
	pub presentation_succeeded: bool,
	/// View settlement classification, including skipped presentation.
	pub presentation_status: PresentationStatus,
	/// Details of abnormal handler termination when available.
	pub execution_failure: Option<crate::execution::ExecutionFailure>,
	/// CLI-specific completion policy when supplied by the view.
	pub cli_completion: Option<crate::application::CliCompletion>,
	detail: Box<dyn ReportDetail>,
}

impl DispatchReport {
	pub(crate) fn from_report<
		O: Send + 'static,
		E: Send + 'static,
		V: std::error::Error + Send + Sync + 'static,
		C: Send + 'static,
	>(
		report: Report<O, E, V, C>,
	) -> Self {
		use crate::execution::PresentationState;
		let presentation_status = match &report.presentation {
			PresentationState::Completed(_) => PresentationStatus::Completed,
			PresentationState::Failed(_) => PresentationStatus::Failed,
			PresentationState::Skipped => PresentationStatus::Skipped,
		};

		let cli_completion = match &report.presentation {
			PresentationState::Completed(value) => (value as &dyn Any)
				.downcast_ref::<crate::application::CliCompletion>()
				.copied(),
			_ => None,
		};

		Self {
			diagnostics: Default::default(),
			domain_succeeded: report.outcome.is_ok(),
			presentation_succeeded: report.presentation.is_ok(),
			presentation_status,
			execution_failure: match &report.outcome {
				crate::execution::ExecutionOutcome::Failed(failure) => Some(failure.clone()),
				_ => None,
			},
			cli_completion,
			detail: Box::new(report),
		}
	}

	/// Classify a view failure, preserving an explicit framework classification where available.
	pub fn presentation_failure(&self) -> Option<crate::error::FailureKind> {
		if self.presentation_status != PresentationStatus::Failed {
			return None;
		}

		Some(match self.detail.failure() {
			Some(error) => error
				.downcast_ref::<crate::error::Failure>()
				.map_or(crate::error::FailureKind::OutputFailed, |e| e.kind),
			None => crate::error::FailureKind::Panicked,
		})
	}

	/// Borrow the underlying view error, when the view returned one.
	pub fn presentation_cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
		self.detail.failure()
	}

	/// Choose the process exit status. Execution or presentation failure returns 1. After normal
	/// settlement, an explicit CLI exit code takes precedence over the domain default.
	pub fn exit_code(&self) -> u8 {
		if self.execution_failure.is_some()
			|| self.presentation_status == PresentationStatus::Failed
		{
			return 1;
		}

		self.cli_completion
			.and_then(|c| c.exit_code)
			.unwrap_or(u8::from(!self.domain_succeeded))
	}

	/// Borrow the typed report for a view returning unit, or `None` for a type mismatch.
	pub fn typed<O: 'static, E: 'static, V: 'static>(&self) -> Option<&Report<O, E, V>> {
		self.detail.as_any().downcast_ref()
	}

	/// Borrow the typed report including the view's completion type, or `None` for a type
	/// mismatch.
	pub fn typed_completion<O: 'static, E: 'static, V: 'static, C: 'static>(
		&self,
	) -> Option<&Report<O, E, V, C>> {
		self.detail.as_any().downcast_ref()
	}
}
