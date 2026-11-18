//! Clap definitions and occurrence-ordered normalization into typed input.

use std::{collections::BTreeMap, ffi::OsString};

use super::input::{FieldValue, InputFields};
use crate::NativePath;
use clap::{Arg, ArgAction, Command, error::ErrorKind};
use serde_json::Value;

use super::{Alias, Bindings, CliError, Parsed, bindings::Binding, schema::Scalar};

const ALTERNATIVE_POSITIONAL: &str = "alternative-positional";

pub(super) fn alias_command(command: &Command, alias: &Alias) -> Command {
	let mut command = command.clone().name(alias.name.clone());
	if let Some(description) = &alias.description {
		command = command.about(description.clone());
	}

	command.mut_args(|argument| {
		let Some(mut field) = argument.get::<super::completion::Field>().cloned() else {
			return argument;
		};
		let Some((_, value)) = alias
			.assignments
			.iter()
			.find(|(name, _)| name == &field.name)
		else {
			return argument;
		};

		field.assigned = Some(
			value
				.as_str()
				.map(str::to_owned)
				.unwrap_or_else(|| value.to_string())
				.into(),
		);
		let named_option = argument.get_long().is_some();
		argument.add(field).required(false).hide(named_option)
	})
}

pub(super) fn build_command(
	name: &str,
	description: &str,
	bindings: &Bindings,
	fields: &BTreeMap<String, Scalar>,
	schema: &Value,
) -> Command {
	let mut command = Command::new(name.to_owned()).about(description.to_owned());
	for binding in &bindings.fields {
		// Append retains every occurrence. Normalization collects string arrays
		// and grouped rows, while rejecting duplicate scalar fields.
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
		if let Some(help) = schema["properties"][&binding.field]["description"].as_str() {
			argument = argument.help(help.to_owned());
		}
		if let Some(long) = &binding.long {
			argument = argument.long(long.clone());
		}

		argument = match fields[&binding.field] {
			Scalar::String | Scalar::StringList => argument
				.num_args(1)
				.value_parser(clap::builder::StringValueParser::new()),
			Scalar::NativePath => argument
				.value_hint(clap::ValueHint::AnyPath)
				.num_args(1)
				.value_parser(clap::builder::OsStringValueParser::new()),
			Scalar::Boolean => argument
				.hide_possible_values(true)
				.num_args(0..=1)
				.require_equals(true)
				.default_missing_value("true")
				.value_parser(clap::value_parser!(bool)),
		};
		if let Some(provider) = bindings.completions.get(&binding.field) {
			argument = argument.add(provider.clone());
		}
		let group = bindings.group.as_ref().and_then(|(group, anchor)| {
			let child = binding.field.strip_prefix(&format!("{group}[]."))?;
			Some(super::completion::GroupField {
				name: group.clone(),
				child: child.into(),
				anchor: child == anchor,
			})
		});
		argument = argument.add(super::completion::Field {
			name: binding.field.clone(),
			group,
			repeated: fields[&binding.field] == Scalar::StringList,
			assigned: None,
		});

		if binding.long.is_none() {
			let required = schema["required"]
				.as_array()
				.is_some_and(|names| names.iter().any(|name| name == &binding.field));
			argument = argument.action(ArgAction::Set).required(required);
		}
		if let Some((field, name)) = &bindings.positional_alternative
			&& field == &binding.field
		{
			let positional = argument
				.clone()
				.id(ALTERNATIVE_POSITIONAL)
				.long(None::<&str>)
				.value_name(name.clone())
				.action(ArgAction::Set)
				.conflicts_with(argument.get_id().clone());
			command = command.arg(positional);
		}

		command = command.arg(argument);
	}

	if let Some(examples) = &bindings.examples {
		command = command.after_help(examples.clone());
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

		let values: Vec<FieldValue> = match fields[&binding.field] {
			Scalar::String => matches
				.get_many::<String>(&id)
				.into_iter()
				.flatten()
				.cloned()
				.map(|value| FieldValue::Scalar(Value::String(value)))
				.collect(),
			Scalar::StringList => matches
				.get_many::<String>(&id)
				.into_iter()
				.flatten()
				.cloned()
				.map(FieldValue::RepeatedString)
				.collect(),
			Scalar::NativePath => matches
				.get_many::<OsString>(&id)
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
				.get_many::<bool>(&id)
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
	if let FieldValue::RepeatedString(value) = value {
		let values = object
			.scalars
			.entry(field.to_owned())
			.or_insert_with(|| Value::Array(Vec::new()));
		values
			.as_array_mut()
			.expect("validated string array binding")
			.push(Value::String(value));
		return Ok(());
	}

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
		FieldValue::RepeatedString(_) => unreachable!("handled string array above"),
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
	// Completion generators can expose IDs as shell syntax, so keep them free of
	// separators and quoting characters. Encode bytes rather than replacing them
	// to preserve distinct schema field names (including the escape marker).
	let mut id = String::from("input-");
	for byte in field.bytes() {
		if byte.is_ascii_alphanumeric() || byte == b'-' {
			id.push(char::from(byte));
		} else {
			use std::fmt::Write;
			write!(id, "_{byte:02x}").expect("writing to a String cannot fail");
		}
	}
	id
}

#[cfg(test)]
mod tests {
	use super::argument_id;
	use std::collections::BTreeSet;

	#[test]
	fn argument_ids_are_shell_safe_without_collapsing_schema_fields() {
		let fields = [
			"task",
			"a:b",
			"a_b",
			"a_3ab",
			"a-b",
			"repo[].path",
			"a'b",
			"路径",
		];
		let ids: BTreeSet<_> = fields.into_iter().map(argument_id).collect();

		assert_eq!(argument_id("task"), "input-task");
		assert_eq!(ids.len(), fields.len());
		assert!(ids.iter().all(|id| {
			id.bytes()
				.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
		}));
	}
}
