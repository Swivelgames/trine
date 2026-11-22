//! Decode matched CLI arguments into fields, preserving their command-line order.

use std::{collections::BTreeMap, ffi::OsString};

use crate::NativePath;
use clap::{Command, error::ErrorKind};
use serde_json::Value;

use super::{
	Alias, Bindings, CliError, Parsed,
	bindings::Binding,
	command::{ALTERNATIVE_POSITIONAL, argument_id},
	input::{FieldValue, InputFields},
	schema::Scalar,
};

struct Occurrence<'a> {
	index: usize,
	binding: &'a Binding,
	value: FieldValue,
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
) -> Result<Vec<Occurrence<'a>>, CliError> {
	let mut occurrences = Vec::new();
	for binding in &bindings.fields {
		let mut id = argument_id(&binding.field);
		if bindings
			.positional_alternative
			.as_ref()
			.is_some_and(|(field, _)| field == &binding.field)
			&& matches.contains_id(ALTERNATIVE_POSITIONAL)
		{
			id = ALTERNATIVE_POSITIONAL.into();
		}
		let Some(indices) = matches.indices_of(&id) else {
			continue;
		};

		let values = matched_values(matches, &id, binding, fields[&binding.field])?;

		for (index, value) in indices.zip(values) {
			occurrences.push(Occurrence {
				index,
				binding,
				value,
			});
		}
	}

	occurrences.sort_by_key(|occurrence| occurrence.index);
	Ok(occurrences)
}

fn matched_values(
	matches: &clap::ArgMatches,
	id: &str,
	binding: &Binding,
	scalar: Scalar,
) -> Result<Vec<FieldValue>, CliError> {
	Ok(match scalar {
		Scalar::String => matches
			.get_many::<String>(id)
			.into_iter()
			.flatten()
			.cloned()
			.map(|value| FieldValue::Scalar(Value::String(value)))
			.collect(),
		Scalar::StringList => matches
			.get_many::<String>(id)
			.into_iter()
			.flatten()
			.cloned()
			.map(FieldValue::RepeatedString)
			.collect(),
		scalar @ (Scalar::NativePath | Scalar::NativePathList) => matches
			.get_many::<OsString>(id)
			.into_iter()
			.flatten()
			.cloned()
			.map(|path| {
				let path = NativePath::new(path)
					.map_err(|error| CliError(format!("{}: {error}", label(binding))))?;
				Ok(if scalar.repeated() {
					FieldValue::RepeatedPath(path)
				} else {
					FieldValue::Path(path)
				})
			})
			.collect::<Result<Vec<_>, CliError>>()?,
		Scalar::Boolean => matches
			.get_many::<bool>(id)
			.into_iter()
			.flatten()
			.copied()
			.map(|value| FieldValue::Scalar(Value::Bool(value)))
			.collect(),
	})
}

fn assemble_fields(
	bindings: &Bindings,
	occurrences: Vec<Occurrence<'_>>,
) -> Result<InputFields, CliError> {
	let mut input = InputFields::default();
	let mut rows: Vec<InputFields> = Vec::new();
	for Occurrence { binding, value, .. } in occurrences {
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
	match value {
		FieldValue::RepeatedString(value) => {
			let values = object
				.scalars
				.entry(field.to_owned())
				.or_insert_with(|| Value::Array(Vec::new()));
			values
				.as_array_mut()
				.expect("validated string list")
				.push(Value::String(value));
		}
		FieldValue::RepeatedPath(path) => {
			object
				.path_lists
				.entry(field.to_owned())
				.or_default()
				.push(path);
		}
		_ if object.scalars.contains_key(field) || object.paths.contains_key(field) => {
			return Err(CliError(format!(
				"{} was supplied more than once for '{}'",
				label(binding),
				binding.field
			)));
		}
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
