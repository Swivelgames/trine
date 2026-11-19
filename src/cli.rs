//! Schema-assisted CLI parsing and a small Smol-hosted presentation runner.
//!
//! Parsing (including help/version) is separate from execution. Bindings are
//! explicit: the schema supplies types, not pluralization or command design.

mod bindings;
pub mod completion;
mod input;
pub mod output_mode;
mod parser;
mod schema;

use std::{ffi::OsString, fmt, num::NonZeroUsize};

use clap::Command;
use serde::de::DeserializeOwned;

use crate::{Action, Cancellation, InvocationContext, RegistrationError, action::validate_name};
use bindings::{validate_aliases, validate_bindings, validate_positionals};
use parser::{build_command, parse};
use schema::Shape;

pub use bindings::{Alias, Bindings};
pub use input::InputFields;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl fmt::Display for CliError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

impl std::error::Error for CliError {}

/// Help and version are terminal parse outcomes, before runtime startup.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed<Input> {
	Input(Input),
	Display(String),
}

type Normalizer<Input> = dyn Fn(Vec<OsString>) -> Result<Parsed<Input>, CliError> + Send + Sync;

/// One typed action and its aliases. Register this assembled CLI in `Registry`
/// to combine different action types and place them under command namespaces.
pub struct Cli<Input, Output, Error, Event> {
	pub(crate) action: Action<Input, Output, Error, Event>,
	normalize: Box<Normalizer<Input>>,
	pub(crate) commands: Vec<Command>,
}

impl<Input, Output, Error, Event> Cli<Input, Output, Error, Event> {
	pub fn new(
		program: impl Into<String>,
		version: impl Into<String>,
		action: Action<Input, Output, Error, Event>,
		bindings: Bindings,
		aliases: Vec<Alias>,
	) -> Result<Self, RegistrationError>
	where
		Input: DeserializeOwned + 'static,
	{
		Self::with_input(
			program,
			version,
			action,
			bindings,
			aliases,
			InputFields::deserialize,
		)
	}

	/// Override typed input assembly when ordinary Serde deserialization is insufficient.
	pub fn with_input(
		program: impl Into<String>,
		version: impl Into<String>,
		action: Action<Input, Output, Error, Event>,
		bindings: Bindings,
		aliases: Vec<Alias>,
		assemble: impl Fn(InputFields) -> Result<Input, CliError> + Send + Sync + 'static,
	) -> Result<Self, RegistrationError>
	where
		Input: 'static,
	{
		let shape = Shape::read(action.input_schema())?;
		let fields = shape.fields();
		validate_bindings(&bindings, &shape, &fields)?;
		validate_aliases(action.name(), &aliases, &shape)?;

		let program = program.into();
		validate_name(&program)?;

		let mut command = Command::new(program)
			.version(version.into())
			.subcommand_required(true);
		let action_command = build_command(
			action.name(),
			action.description(),
			&bindings,
			&fields,
			action.input_schema(),
		);
		command = command.subcommand(action_command.clone());
		for alias in &aliases {
			command = command.subcommand(parser::alias_command(&action_command, alias));
		}

		for route in command.get_subcommands() {
			validate_positionals(route)?;
		}

		let commands = command.get_subcommands().cloned().collect();
		let normalize =
			move |arguments| match parse(&command, &bindings, &fields, &aliases, arguments)? {
				Parsed::Display(text) => Ok(Parsed::Display(text)),
				Parsed::Input(fields) => assemble(fields).map(Parsed::Input),
			};

		Ok(Self {
			action,
			normalize: Box::new(normalize),
			commands,
		})
	}

	/// Escape hatch for schemas or native-path handling outside the initial
	/// translator. The callback receives untouched OS arguments (including argv0)
	/// and owns help/version and input normalization. No runtime is started.
	pub fn custom(
		action: Action<Input, Output, Error, Event>,
		normalize: impl Fn(Vec<OsString>) -> Result<Parsed<Input>, CliError> + Send + Sync + 'static,
	) -> Self {
		Self {
			action,
			normalize: Box::new(normalize),
			commands: Vec::new(),
		}
	}

	pub fn parse(
		&self,
		arguments: impl IntoIterator<Item = impl Into<OsString>>,
	) -> Result<Parsed<Input>, CliError> {
		(self.normalize)(arguments.into_iter().map(Into::into).collect())
	}

	/// Run one action and its progress consumer together. Accepted events drain
	/// before the result is returned; terminal presentation belongs to the caller.
	/// Cancellation is cooperative and the handler is always awaited.
	pub fn run(
		&self,
		input: Input,
		capacity: NonZeroUsize,
		cancellation: Cancellation,
		mut render_event: impl FnMut(Event),
	) -> Result<Output, Error> {
		let (context, events) = InvocationContext::observed(capacity, cancellation);

		smol::block_on(async {
			let consume = async {
				while let Ok(event) = events.recv().await {
					render_event(event);
				}
			};

			let (result, ()) =
				futures_lite::future::zip(self.action.invoke(input, context), consume).await;
			result
		})
	}
}
