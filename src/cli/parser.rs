//! Clap definitions and occurrence-ordered normalization into typed input.

use std::{collections::BTreeMap, ffi::OsString};

use super::input::{FieldValue, InputFields};
use crate::NativePath;
use clap::{Arg, ArgAction, Command, error::ErrorKind};
use serde_json::Value;

use super::{Alias, Bindings, CliError, Parsed, bindings::Binding, schema::Scalar};

pub(super) fn build_command(
	name: &str,
	description: &str,
	bindings: &Bindings,
	fields: &BTreeMap<String, Scalar>,
) -> Command {
	let mut command = Command::new(name.to_owned()).about(description.to_owned());
	for binding in &bindings.fields {
		// Append retains every occurrence. Normalization rejects duplicate fields
		// while allowing a group's child to recur for separate repository rows.
		let mut argument = Arg::new(argument_id(&binding.field))
			.value_name(
				binding
					.field
					.rsplit('.')
					.next()
					.unwrap()
					.to_ascii_uppercase(),
			)
			.action(ArgAction::Append);
		if let Some(long) = &binding.long {
			argument = argument.long(long.clone());
		}

		argument = match fields[&binding.field] {
			Scalar::String => argument
				.num_args(1)
				.value_parser(clap::builder::StringValueParser::new()),
			Scalar::NativePath => argument
				.num_args(1)
				.value_parser(clap::builder::OsStringValueParser::new()),
			Scalar::Boolean => argument
				.num_args(0..=1)
				.require_equals(true)
				.default_missing_value("true")
				.value_parser(clap::value_parser!(bool)),
		};
		command = command.arg(argument);
	}

	command
}

pub(super) fn parse(
	command: &Command,
	bindings: &Bindings,
	fields: &BTreeMap<String, Scalar>,
	aliases: &[Alias],
	arguments: Vec<OsString>,
) -> Result<Parsed<InputFields>, CliError> {
	let matches = match command.clone().try_get_matches_from(arguments) {
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

	let (name, matches) = matches.subcommand().expect("Clap requires a subcommand");
	let occurrences = collect_occurrences(matches, bindings, fields)?;
	let mut input = assemble_fields(bindings, occurrences)?;
	apply_alias(&mut input, aliases, name)?;
	Ok(Parsed::Input(input))
}

fn collect_occurrences<'a>(
	matches: &clap::ArgMatches,
	bindings: &'a Bindings,
	fields: &BTreeMap<String, Scalar>,
) -> Result<Vec<(usize, &'a Binding, FieldValue)>, CliError> {
	let mut occurrences = Vec::new();
	for binding in &bindings.fields {
		let Some(indices) = matches.indices_of(&argument_id(&binding.field)) else {
			continue;
		};

		let values: Vec<FieldValue> = match fields[&binding.field] {
			Scalar::String => matches
				.get_many::<String>(&argument_id(&binding.field))
				.into_iter()
				.flatten()
				.cloned()
				.map(|value| FieldValue::Scalar(Value::String(value)))
				.collect(),
			Scalar::NativePath => matches
				.get_many::<OsString>(&argument_id(&binding.field))
				.into_iter()
				.flatten()
				.cloned()
				.map(|path| {
					NativePath::new(path)
						.map(FieldValue::Path)
						.map_err(|error| CliError(format!("{}: {error}", label(binding))))
				})
				.collect::<Result<Vec<_>, _>>()?,
			Scalar::Boolean => matches
				.get_many::<bool>(&argument_id(&binding.field))
				.into_iter()
				.flatten()
				.copied()
				.map(|value| FieldValue::Scalar(Value::Bool(value)))
				.collect(),
		};

		for (index, value) in indices.zip(values) {
			occurrences.push((index, binding, value));
		}
	}

	occurrences.sort_by_key(|(index, _, _)| *index);
	Ok(occurrences)
}

fn assemble_fields(
	bindings: &Bindings,
	occurrences: Vec<(usize, &Binding, FieldValue)>,
) -> Result<InputFields, CliError> {
	let mut input = InputFields::default();
	let mut rows: Vec<InputFields> = Vec::new();
	for (_, binding, value) in occurrences {
		let Some((group, field)) = binding.field.split_once("[].") else {
			insert(&mut input, &binding.field, value, binding)?;
			continue;
		};

		let (expected_group, anchor) = bindings.group.as_ref().expect("validated group");
		debug_assert_eq!(group, expected_group);
		if field == anchor {
			rows.push(InputFields::default());
		}

		let row = rows.last_mut().ok_or_else(|| {
			CliError(format!(
				"{} requires a preceding group anchor for '{group}[].{anchor}'",
				label(binding)
			))
		})?;
		insert(row, field, value, binding)?;
	}

	if let Some((group, _)) = &bindings.group
		&& !rows.is_empty()
	{
		input.groups.insert(group.clone(), rows);
	}
	Ok(input)
}

fn apply_alias(input: &mut InputFields, aliases: &[Alias], name: &str) -> Result<(), CliError> {
	let Some(alias) = aliases.iter().find(|alias| alias.name == name) else {
		return Ok(());
	};

	for (field, value) in &alias.assignments {
		if input
			.scalars
			.get(field)
			.is_some_and(|existing| existing != value)
		{
			return Err(CliError(format!("'{field}' conflicts with alias '{name}'")));
		}
		input.scalars.insert(field.clone(), value.clone());
	}
	Ok(())
}

fn insert(
	object: &mut InputFields,
	field: &str,
	value: FieldValue,
	binding: &Binding,
) -> Result<(), CliError> {
	if object.scalars.contains_key(field) || object.paths.contains_key(field) {
		return Err(CliError(format!(
			"{} was supplied more than once for '{}'",
			label(binding),
			binding.field
		)));
	}

	match value {
		FieldValue::Scalar(value) => {
			object.scalars.insert(field.to_owned(), value);
		}
		FieldValue::Path(path) => {
			object.paths.insert(field.to_owned(), path);
		}
	}
	Ok(())
}

fn label(binding: &Binding) -> String {
	binding.long.as_ref().map_or_else(
		|| format!("positional '{}'", binding.field),
		|long| format!("--{long}"),
	)
}

fn argument_id(field: &str) -> String {
	format!("input:{field}")
}
