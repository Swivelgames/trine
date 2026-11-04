//! Assemble typed entries before erasing their command-dispatch boundary.

use crate::{
	Cancellation, RegistrationError,
	action::validate_name,
	cli::{Cli, CliError, Parsed},
	view::{self, Report, View},
};
use clap::{Command, error::ErrorKind};
use std::{any::Any, collections::BTreeMap, ffi::OsString, num::NonZeroUsize, sync::Arc};

type Prepare = dyn Fn(Vec<OsString>) -> Result<Prepared, CliError> + Send + Sync;

struct Entry {
	command: Command,
	prepare: Arc<Prepare>,
}

pub struct Registry {
	program: String,
	version: String,
	entries: BTreeMap<Vec<String>, Entry>,
}

impl Registry {
	pub fn new(
		program: impl Into<String>,
		version: impl Into<String>,
	) -> Result<Self, RegistrationError> {
		let program = program.into();
		validate_name(&program)?;

		Ok(Self {
			program,
			version: version.into(),
			entries: BTreeMap::new(),
		})
	}

	/// Path identifies the primary action; aliases share its parent namespace.
	pub fn register<I, O, E, P, V>(
		&mut self,
		path: impl IntoIterator<Item = impl Into<String>>,
		cli: Cli<I, O, E, P>,
		factory: impl Fn() -> V + Send + Sync + 'static,
		capacity: Option<NonZeroUsize>,
	) -> Result<(), RegistrationError>
	where
		I: Send + 'static,
		O: Send + Sync + 'static,
		E: Send + Sync + 'static,
		P: Send + 'static,
		V: View<O, E, P> + 'static,
		V::Error: std::error::Error + Send + Sync + 'static,
		V::Completion: 'static,
	{
		let path: Vec<String> = path.into_iter().map(Into::into).collect();
		if path.is_empty() || cli.commands.is_empty() {
			return Err(RegistrationError(
				"registry entry needs a command path and schema-assisted CLI metadata".into(),
			));
		}
		for segment in &path {
			validate_name(segment)?;
			if segment == "help" {
				return Err(RegistrationError("'help' is reserved".into()));
			}
		}

		let mut routes = Vec::new();
		for command in &cli.commands {
			let mut route = path.clone();
			if command.get_name() != cli.action.name() {
				*route.last_mut().unwrap() = command.get_name().into();
			}
			if self
				.entries
				.keys()
				.chain(routes.iter().map(|(route, _)| route))
				.any(|existing| conflicts(existing, &route))
			{
				return Err(RegistrationError(format!(
					"duplicate command, alias or leaf-prefix conflict: {}",
					route.join(" ")
				)));
			}
			routes.push((route, command.clone()));
		}

		let cli = Arc::new(cli);
		let factory = Arc::new(factory);
		for (route, command) in routes {
			let cli = cli.clone();
			let factory = factory.clone();
			let original_name = command.get_name().to_owned();
			let command = command.name(route.last().unwrap().clone());

			let prepare = move |mut arguments: Vec<OsString>| {
				arguments.insert(0, original_name.clone().into());
				arguments.insert(0, "trine".into());
				let Parsed::Input(input) = cli.parse(arguments)? else {
					return Err(CliError(
						"expected invocation after registry parsing".into(),
					));
				};

				let cli = cli.clone();
				let factory = factory.clone();
				Ok(Prepared(Box::new(move |cancellation| {
					let report = view::run(&cli.action, input, factory(), capacity, cancellation);
					DispatchReport::from_report(report)
				})))
			};

			self.entries.insert(
				route,
				Entry {
					command,
					prepare: Arc::new(prepare),
				},
			);
		}
		Ok(())
	}

	pub fn parse(
		&self,
		arguments: impl IntoIterator<Item = impl Into<OsString>>,
	) -> Result<Parsed<Prepared>, CliError> {
		let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
		let command = self
			.command_tree(&[])
			.name(self.program.clone())
			.version(self.version.clone());
		let matches = match command.try_get_matches_from(arguments.clone()) {
			Ok(matches) => matches,
			Err(error)
				if matches!(
					error.kind(),
					ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
				) =>
			{
				return Ok(Parsed::Display(error.to_string()));
			}
			Err(error) => return Err(CliError(error.to_string())),
		};

		let mut path = Vec::new();
		let mut cursor = &matches;
		while let Some((name, matches)) = cursor.subcommand() {
			path.push(name.to_owned());
			cursor = matches;
		}

		let entry = self
			.entries
			.get(&path)
			.ok_or_else(|| CliError("choose an action".into()))?;

		(entry.prepare)(arguments.into_iter().skip(path.len() + 1).collect()).map(Parsed::Input)
	}

	fn command_tree(&self, prefix: &[String]) -> Command {
		if let Some(entry) = self.entries.get(prefix) {
			return entry.command.clone();
		}

		let mut command = Command::new(
			prefix
				.last()
				.cloned()
				.unwrap_or_else(|| self.program.clone()),
		)
		.subcommand_required(true);

		let mut children = std::collections::BTreeSet::new();
		for path in self.entries.keys().filter(|path| path.starts_with(prefix)) {
			if let Some(child) = path.get(prefix.len()) {
				children.insert(child.clone());
			}
		}

		for child in children {
			let mut path = prefix.to_vec();
			path.push(child);
			command = command.subcommand(self.command_tree(&path));
		}

		command
	}
}

fn conflicts(first: &[String], second: &[String]) -> bool {
	first.starts_with(second) || second.starts_with(first)
}

pub struct Prepared(Box<dyn FnOnce(Cancellation) -> DispatchReport + Send>);

impl Prepared {
	pub fn run(self, cancellation: Cancellation) -> DispatchReport {
		(self.0)(cancellation)
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationStatus {
	Completed,
	Failed,
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

pub struct DispatchReport {
	pub diagnostics: crate::diagnostics::DiagnosticDelivery,
	pub domain_succeeded: bool,
	pub presentation_succeeded: bool,
	pub presentation_status: PresentationStatus,
	pub execution_failure: Option<crate::execution::ExecutionFailure>,
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

	pub fn presentation_cause(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
		self.detail.failure()
	}

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

	pub fn typed<O: 'static, E: 'static, V: 'static>(&self) -> Option<&Report<O, E, V>> {
		self.detail.as_any().downcast_ref()
	}

	pub fn typed_completion<O: 'static, E: 'static, V: 'static, C: 'static>(
		&self,
	) -> Option<&Report<O, E, V, C>> {
		self.detail.as_any().downcast_ref()
	}
}
