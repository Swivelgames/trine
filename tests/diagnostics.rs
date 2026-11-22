//! Diagnostic delivery and fallback when renderers or writers fail.

use std::cell::Cell;
use trine::{
	diagnostics::{Diagnostic, DiagnosticRenderer, deliver},
	error::{Failure, FailureKind},
};

fn diagnostic() -> Diagnostic {
	Diagnostic {
		execution_failure: Some("handler panicked".into()),
		domain_returned_ok: false,
		presentation_failure: None,
		presentation_message: None,
	}
}

#[test]
fn successful_custom_diagnostic_needs_no_fallback() {
	let renderer = DiagnosticRenderer::new(|_| Ok("custom diagnostic".into()));
	let report = deliver(
		&diagnostic(),
		&renderer,
		|text| {
			assert_eq!(text, "custom diagnostic");
			Ok(())
		},
		|_| panic!("fallback must not run"),
	);
	assert!(report.attempted);
	assert!(report.failure.is_none());
	assert!(!report.fallback_attempted);
}

#[test]
fn renderer_or_writer_failure_gets_one_bounded_fallback_even_if_it_panics() {
	for renderer_panics in [false, true] {
		let renderer = DiagnosticRenderer::new(move |_| {
			if renderer_panics {
				panic!("renderer");
			}
			Ok("diagnostic".into())
		});
		let fallback_calls = Cell::new(0);
		let report = deliver(
			&diagnostic(),
			&renderer,
			|_| Err(Failure::message(FailureKind::OutputFailed, "writer failed")),
			|text| {
				fallback_calls.set(fallback_calls.get() + 1);
				assert!(text.contains("effects may be incomplete"));
				panic!("fallback writer");
			},
		);
		assert_eq!(fallback_calls.get(), 1);
		assert_eq!(
			report.failure.unwrap().kind,
			if renderer_panics {
				FailureKind::Panicked
			} else {
				FailureKind::OutputFailed
			}
		);
		assert_eq!(report.fallback_failure.unwrap().kind, FailureKind::Panicked);
	}
}

#[test]
fn primary_writer_panic_is_caught_and_fallback_can_succeed() {
	let report = deliver(
		&diagnostic(),
		&DiagnosticRenderer::default(),
		|_| panic!("writer panic"),
		|_| Ok(()),
	);
	assert_eq!(report.failure.unwrap().kind, FailureKind::Panicked);
	assert!(report.fallback_attempted);
	assert!(report.fallback_failure.is_none());
}

#[derive(Debug)]
struct PanickingDisplay;
impl std::fmt::Display for PanickingDisplay {
	fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		panic!("error Display panicked");
	}
}
impl std::error::Error for PanickingDisplay {}

#[test]
fn primary_error_formatting_panic_retains_error_and_delivers_literal_fallback() {
	use std::error::Error;
	let report = deliver(
		&diagnostic(),
		&DiagnosticRenderer::default(),
		|_| Err(Failure::new(FailureKind::OutputFailed, PanickingDisplay)),
		|text| {
			assert_eq!(
				text,
				"Framework diagnostic delivery failed; error formatting also panicked."
			);
			Ok(())
		},
	);
	let failure = report.failure.unwrap();
	assert_eq!(failure.kind, FailureKind::OutputFailed);
	assert!(failure.source().unwrap().is::<PanickingDisplay>());
	assert!(report.fallback_attempted);
	assert!(report.fallback_failure.is_none());
}

#[test]
fn presentation_error_formatting_panic_preserves_settled_report() {
	use trine::{Action, Application, Cancellation, Entry, InvocationContext, view::View};
	struct BrokenView;
	impl View<usize, (), ()> for BrokenView {
		type Completion = ();
		type Error = PanickingDisplay;
		async fn progress(&mut self, _: ()) -> Result<(), Self::Error> {
			Ok(())
		}
		async fn finish(&mut self, _: &Result<usize, ()>) -> Result<(), Self::Error> {
			Err(PanickingDisplay)
		}
	}
	let action = Action::new("run", "Run", |_: (), _: InvocationContext<()>| async {
		Ok::<_, ()>(7)
	})
	.unwrap();
	let mut app = Application::new("demo", "1").unwrap();
	app.register(
		Entry::new(action).rpc(trine::structured::StructuredBinding::custom(|_| {
			Ok(BrokenView)
		})),
	)
	.unwrap();
	let report = app
		.invoke_rpc(
			&["run"],
			serde_json::Value::Null,
			false,
			Cancellation::new(),
		)
		.unwrap();
	assert!(report.execution.domain_succeeded);
	assert_eq!(
		report.execution.presentation_failure(),
		Some(FailureKind::OutputFailed)
	);
	assert!(
		report
			.execution
			.presentation_cause()
			.unwrap()
			.source()
			.unwrap()
			.is::<PanickingDisplay>()
	);
	assert!(report.execution.diagnostics.attempted);
	assert!(report.execution.diagnostics.failure.is_none());
	assert!(report.transcript.diagnostics[0].contains("presentation error formatting panicked"));
	assert!(matches!(
		report
			.execution
			.typed::<usize, (), Failure>()
			.unwrap()
			.outcome,
		trine::execution::ExecutionOutcome::Completed(Ok(7))
	));
}
