//! CLI binding, parsing metadata and invocation view assembly.

mod execution;

use crate::application::json::JsonProjection;
use crate::application::{CliRegistration, CliViewContext, InvocationError, PreparedCli};
use crate::{
	Action, Cancellation,
	cli::{Alias, Bindings, Cli, CliError, InputFields, Parsed},
	diagnostics::DiagnosticRenderer,
	error::Failure,
	view::{ErasedView, View},
};
use std::{num::NonZeroUsize, sync::Arc};

/// CLI argument mappings, aliases and progress-buffer settings for an entry.
pub struct CliBinding {
	bindings: Bindings,
	aliases: Vec<Alias>,
	capacity: Option<NonZeroUsize>,
}

impl CliBinding {
	/// Start with the supplied argument mappings and a 16-event progress buffer.
	pub fn new(bindings: Bindings) -> Self {
		Self {
			bindings,
			aliases: vec![],
			capacity: NonZeroUsize::new(16),
		}
	}

	/// Add another command spelling for the same action.
	pub fn alias(mut self, alias: Alias) -> Self {
		self.aliases.push(alias);
		self
	}

	/// Set the maximum number of buffered CLI progress events. Pass `None` to disable observation.
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
	projection: JsonProjection<O, E>,
) -> Result<CliRegistration, crate::RegistrationError>
where
	I: Send + 'static,
	O: Send + Sync + 'static,
	E: Send + Sync + 'static,
	P: Send + 'static,
	V: View<O, E, P, Completion = ()> + 'static,
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
		let projection = projection.clone();
		let prepare_view = move |context: CliViewContext,
		                         json: bool,
		                         cancellation: Cancellation,
		                         renderer: DiagnosticRenderer| {
			if json {
				return Ok(execution::prepare_json(
					cli,
					input,
					projection,
					context,
					cancellation,
					renderer,
				));
			}

			let stderr = context.stderr.clone();
			let view = factory(context).map_err(InvocationError::Setup)?;
			let view = ErasedView::new(projection.human(view));
			Ok(execution::prepare_human(
				cli,
				input,
				view,
				stderr,
				cancellation,
				renderer,
				binding.capacity,
			))
		};

		Ok(Parsed::Input(PreparedCli {
			prepare: Box::new(prepare_view),
			json: false,
			renderer: Default::default(),
		}))
	});

	Ok(CliRegistration { commands, prepare })
}
