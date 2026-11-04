//! Typed assembly before catalog erasure.

use super::{
	CliBinding, CliBuild, CliRegistration, CliViewContext, InvocationError, PreparedCli,
	StructuredPrepare, presentation,
};
use crate::error::{Failure, FailureKind};
use crate::{
	Action,
	cli::{Cli, CliError, InputFields, Parsed},
	registry::DispatchReport,
	structured::{GuardedView, StructuredBinding, StructuredContext},
	view::{self, View},
};
use serde::de::DeserializeOwned;
use std::sync::Arc;

/// Interface methods check their own bounds before the completed entry is erased.
pub struct Entry<I, O, E, P> {
	pub(super) action: Action<I, O, E, P>,
	pub(super) namespace: Vec<String>,
	pub(super) cli: Option<Box<CliBuild>>,
	pub(super) rpc: Option<Arc<StructuredPrepare>>,
	pub(super) mcp: Option<Arc<StructuredPrepare>>,
	pub(super) duplicate: bool,
}

impl<I, O, E, P> Entry<I, O, E, P> {
	pub fn new(action: Action<I, O, E, P>) -> Self {
		Self {
			action,
			namespace: vec![],
			cli: None,
			rpc: None,
			mcp: None,
			duplicate: false,
		}
	}

	pub fn namespace(mut self, segments: impl IntoIterator<Item = impl Into<String>>) -> Self {
		self.namespace = segments.into_iter().map(Into::into).collect();
		self
	}
}

impl<I: Send + 'static, O: Send + Sync + 'static, E: Send + Sync + 'static, P: Send + 'static>
	Entry<I, O, E, P>
{
	pub fn cli<V, F>(self, binding: CliBinding, factory: F) -> Self
	where
		I: DeserializeOwned,
		V: View<O, E, P> + 'static,
		V::Completion: Into<super::CliCompletion>,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
		F: Fn(CliViewContext) -> Result<V, Failure> + Send + Sync + 'static,
	{
		self.cli_with_input(binding, InputFields::deserialize, factory)
	}

	pub fn cli_with_input<V, F, A>(mut self, binding: CliBinding, assemble: A, factory: F) -> Self
	where
		V: View<O, E, P> + 'static,
		V::Completion: Into<super::CliCompletion>,
		V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
		F: Fn(CliViewContext) -> Result<V, Failure> + Send + Sync + 'static,
		A: Fn(InputFields) -> Result<I, CliError> + Send + Sync + 'static,
	{
		self.duplicate |= self.cli.is_some();
		let action = self.action.clone();
		self.cli = Some(Box::new(move |program, version| {
			build_cli_registration(program, version, action, binding, assemble, factory)
		}));
		self
	}

	pub fn rpc(mut self, binding: StructuredBinding<O, E, P>) -> Self
	where
		I: DeserializeOwned,
	{
		self.duplicate |= self.rpc.is_some();
		self.rpc = Some(structured_prepare(self.action.clone(), binding));
		self
	}

	pub fn mcp(mut self, binding: StructuredBinding<O, E, P>) -> Self
	where
		I: DeserializeOwned,
	{
		self.duplicate |= self.mcp.is_some();
		self.mcp = Some(structured_prepare(self.action.clone(), binding));
		self
	}
}

/// Build reusable parsing metadata; neither the view factory nor action runs here.
fn build_cli_registration<I, O, E, P, V, F, A>(
	program: &str,
	version: &str,
	action: Action<I, O, E, P>,
	binding: CliBinding,
	assemble: A,
	factory: F,
) -> Result<CliRegistration, crate::RegistrationError>
where
	I: Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
	V: View<O, E, P> + 'static,
	V::Completion: Into<super::CliCompletion>,
	V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
	F: Fn(CliViewContext) -> Result<V, Failure> + Send + Sync + 'static,
	A: Fn(InputFields) -> Result<I, CliError> + Send + Sync + 'static,
{
	let cli = Arc::new(Cli::with_input(
		program,
		version,
		action,
		binding.bindings,
		binding.aliases,
		assemble,
	)?);

	let commands = cli.commands.clone();
	let factory = Arc::new(factory);

	let prepare = Arc::new(move |arguments| {
		let input = match cli.parse(arguments)? {
			Parsed::Display(text) => return Ok(Parsed::Display(text)),
			Parsed::Input(input) => input,
		};

		let cli = cli.clone();
		let factory = factory.clone();
		let prepare_view =
			move |context: CliViewContext,
			      cancellation: crate::Cancellation,
			      renderer: crate::diagnostics::DiagnosticRenderer| {
				prepare_cli_execution(
					cli,
					input,
					&*factory,
					context,
					cancellation,
					renderer,
					binding.capacity,
				)
			};

		Ok(Parsed::Input(PreparedCli {
			prepare: Box::new(prepare_view),
			renderer: Default::default(),
		}))
	});

	Ok(CliRegistration { commands, prepare })
}

/// Construct the view before handing the host an executable invocation.
fn prepare_cli_execution<I, O, E, P, V>(
	cli: Arc<Cli<I, O, E, P>>,
	input: I,
	factory: &impl Fn(CliViewContext) -> Result<V, Failure>,
	context: CliViewContext,
	cancellation: crate::Cancellation,
	renderer: crate::diagnostics::DiagnosticRenderer,
	capacity: Option<std::num::NonZeroUsize>,
) -> Result<super::PreparedCliExecution, InvocationError>
where
	I: Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
	V: View<O, E, P> + 'static,
	V::Completion: Into<super::CliCompletion>,
	V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
	let stderr = context.stderr.clone();
	let view = factory(context).map_err(InvocationError::Setup)?;
	let view = presentation::ErasedView::<O, E, P, super::CliCompletion>::new(view);

	Ok(super::PreparedCliExecution {
		cancellation: cancellation.clone(),
		run: Box::new(move || {
			let report = view::run(&cli.action, input, view, capacity, cancellation);
			let mut report = DispatchReport::from_report(report);
			deliver_cli_diagnostic(&mut report, &renderer, &stderr);
			report
		}),
	})
}

fn deliver_cli_diagnostic(
	report: &mut DispatchReport,
	renderer: &crate::diagnostics::DiagnosticRenderer,
	stderr: &super::Output,
) {
	let Some(diagnostic) = crate::diagnostics::Diagnostic::from_report(report) else {
		return;
	};

	let write = |text: &str| {
		stderr
			.write_all(format!("{text}\n").as_bytes())
			.map_err(|error| Failure::new(FailureKind::OutputFailed, error))
	};

	report.diagnostics = crate::diagnostics::deliver(&diagnostic, renderer, write, write);
}

fn deliver_structured_diagnostic(
	report: &mut DispatchReport,
	renderer: &crate::diagnostics::DiagnosticRenderer,
	context: &StructuredContext,
) {
	let Some(diagnostic) = crate::diagnostics::Diagnostic::from_report(report) else {
		return;
	};

	report.diagnostics = crate::diagnostics::deliver(
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

fn structured_prepare<I, O, E, P>(
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
		move |live: Option<Arc<dyn crate::structured::LiveSink>>,
		      value,
		      observe,
		      cancellation: crate::Cancellation,
		      renderer: crate::diagnostics::DiagnosticRenderer| {
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

			let invocation = crate::execution::PreparedInvocation::new(
				action.clone(),
				input,
				view,
				capacity,
				cancellation.clone(),
			);

			Ok(super::PreparedStructured {
				cancellation,
				future: Box::pin(async move {
					let _close = close;
					let mut report = invocation.run().await;
					if let Err(error) = context.close(false)
						&& !report.presentation.is_err()
					{
						report.presentation = crate::execution::PresentationState::Failed(
							crate::execution::PresentationFailure::Output(error),
						);
					}

					let mut execution = DispatchReport::from_report(report);
					if !is_live {
						deliver_structured_diagnostic(&mut execution, &renderer, &context);
					}

					super::AsyncStructuredReport {
						execution,
						transcript: context.transcript(),
					}
				}),
			})
		},
	);

	Arc::new(StructuredPrepare { prepare })
}
