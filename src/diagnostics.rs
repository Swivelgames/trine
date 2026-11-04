//! Host diagnostics are separate from typed terminal presentation.

use crate::{
	error::{Failure, FailureKind},
	registry::{DispatchReport, PresentationStatus},
};
use std::{
	panic::{AssertUnwindSafe, catch_unwind},
	sync::Arc,
};

#[derive(Clone, Debug)]
pub struct Diagnostic {
	pub execution_failure: Option<String>,
	pub domain_returned_ok: bool,
	pub presentation_failure: Option<FailureKind>,
	pub presentation_message: Option<String>,
}

impl Diagnostic {
	pub fn from_report(report: &DispatchReport) -> Option<Self> {
		if report.execution_failure.is_none()
			&& report.presentation_status != PresentationStatus::Failed
		{
			return None;
		}

		Some(Self {
			execution_failure: report.execution_failure.as_ref().map(|e| e.message.clone()),
			domain_returned_ok: report.domain_succeeded,
			presentation_failure: report.presentation_failure(),
			presentation_message: report.presentation_cause().map(|error| {
				format_or_literal(
					|| error.to_string(),
					"presentation error formatting panicked",
				)
			}),
		})
	}

	pub fn message(&self) -> String {
		let mut message = match &self.execution_failure {
			Some(error) => format!("Execution terminated: {error}; effects may be incomplete."),
			None if self.domain_returned_ok => {
				"The action returned an outcome, but presentation failed.".into()
			}
			None => "The action returned a domain error, and presentation failed.".into(),
		};

		if let Some(kind) = self.presentation_failure {
			message.push_str(&format!(
				" Presentation {kind:?}: {}",
				self.presentation_message
					.as_deref()
					.unwrap_or("view panicked")
			));
		}

		message
	}
}

type Render = dyn Fn(&Diagnostic) -> Result<String, Failure> + Send + Sync;

#[derive(Clone)]
pub struct DiagnosticRenderer(Arc<Render>);

impl DiagnosticRenderer {
	pub(crate) fn render(&self, diagnostic: &Diagnostic) -> Result<String, Failure> {
		catch_unwind(AssertUnwindSafe(|| (self.0)(diagnostic))).unwrap_or_else(|_| {
			Err(Failure::message(
				FailureKind::Panicked,
				"diagnostic renderer panicked",
			))
		})
	}

	pub fn new(
		render: impl Fn(&Diagnostic) -> Result<String, Failure> + Send + Sync + 'static,
	) -> Self {
		Self(Arc::new(render))
	}
}

impl Default for DiagnosticRenderer {
	fn default() -> Self {
		Self::new(|diagnostic| Ok(diagnostic.message()))
	}
}

#[derive(Clone, Debug, Default)]
pub struct DiagnosticDelivery {
	pub attempted: bool,
	pub failure: Option<Failure>,
	pub fallback_attempted: bool,
	pub fallback_failure: Option<Failure>,
}

// Error Display implementations are user code too. Formatting must not discard
// a settled report or prevent the single fallback attempt.
fn format_or_literal(format: impl FnOnce() -> String, fallback: &str) -> String {
	catch_unwind(AssertUnwindSafe(format)).unwrap_or_else(|_| fallback.to_owned())
}

fn attempt(operation: impl FnOnce() -> Result<(), Failure>) -> Result<(), Failure> {
	catch_unwind(AssertUnwindSafe(operation)).unwrap_or_else(|_| {
		Err(Failure::message(
			FailureKind::Panicked,
			"diagnostic delivery panicked",
		))
	})
}

/// One primary attempt and at most one fallback; neither rewrites execution.
pub fn deliver(
	diagnostic: &Diagnostic,
	renderer: &DiagnosticRenderer,
	write: impl FnOnce(&str) -> Result<(), Failure>,
	fallback: impl FnOnce(&str) -> Result<(), Failure>,
) -> DiagnosticDelivery {
	let primary = attempt(|| write(&(renderer.0)(diagnostic)?));
	let Err(error) = primary else {
		return DiagnosticDelivery {
			attempted: true,
			..Default::default()
		};
	};

	let message = format_or_literal(
		|| {
			format!(
				"{} Diagnostic delivery failed: {error}",
				diagnostic.message()
			)
		},
		"Framework diagnostic delivery failed; error formatting also panicked.",
	);
	let fallback_failure = attempt(|| fallback(&message)).err();

	DiagnosticDelivery {
		attempted: true,
		failure: Some(error),
		fallback_attempted: true,
		fallback_failure,
	}
}
