//! CLI binding, parsing metadata and invocation view assembly.

use crate::application::{
	CliCompletion, CliRegistration, CliViewContext, InvocationError, Output, PreparedCli,
	PreparedCliExecution,
};
use crate::{
	Action, Cancellation,
	cli::{Alias, Bindings, Cli, CliError, InputFields, Parsed},
	diagnostics::{self, Diagnostic, DiagnosticRenderer},
	error::{Failure, FailureKind},
	registry::DispatchReport,
	view::{self, ErasedView, View},
};
use std::{num::NonZeroUsize, sync::Arc};

pub struct CliBinding {
	bindings: Bindings,
	aliases: Vec<Alias>,
	capacity: Option<NonZeroUsize>,
}

impl CliBinding {
	pub fn new(bindings: Bindings) -> Self {
		Self {
			bindings,
			aliases: vec![],
			capacity: NonZeroUsize::new(16),
		}
	}

	pub fn alias(mut self, alias: Alias) -> Self {
		self.aliases.push(alias);
		self
	}

	pub fn capacity(mut self, capacity: Option<NonZeroUsize>) -> Self {
		self.capacity = capacity;
		self
	}
}

/// Build reusable parsing metadata; neither the view factory nor action runs here.
pub(super) fn build_cli_registration<I, O, E, P, V, F, A>(
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
	V::Completion: Into<CliCompletion>,
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
		let prepare_view = move |context: CliViewContext,
		                         cancellation: Cancellation,
		                         renderer: DiagnosticRenderer| {
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
	cancellation: Cancellation,
	renderer: DiagnosticRenderer,
	capacity: Option<NonZeroUsize>,
) -> Result<PreparedCliExecution, InvocationError>
where
	I: Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
	V: View<O, E, P> + 'static,
	V::Completion: Into<CliCompletion>,
	V::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
	let stderr = context.stderr.clone();
	let view = factory(context).map_err(InvocationError::Setup)?;
	let view = ErasedView::<O, E, P, CliCompletion>::new(view);

	Ok(PreparedCliExecution {
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
