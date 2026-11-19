//! The deliberately small schema subset supported by the first CLI adapter.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::RegistrationError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scalar {
	String,
	StringList,
	Boolean,
	NativePath,
	NativePathList,
}

impl Scalar {
	pub fn native(self) -> bool {
		matches!(self, Self::NativePath | Self::NativePathList)
	}

	pub fn repeated(self) -> bool {
		matches!(self, Self::StringList | Self::NativePathList)
	}
}

#[derive(Debug)]
pub(super) struct Group {
	pub field: String,
	pub fields: BTreeMap<String, Scalar>,
}

#[derive(Debug)]
pub(super) struct Shape {
	pub scalars: BTreeMap<String, Scalar>,
	pub group: Option<Group>,
}

impl Shape {
	pub fn read(root: &Value) -> Result<Self, RegistrationError> {
		let object = resolve(root, root, "input")?;
		let properties = object_properties(object, "input")?;
		let mut scalars = BTreeMap::new();
		let mut group = None;

		for (field, schema) in properties {
			let schema = resolve(root, schema, field)?;
			if schema["type"] != "array" {
				scalars.insert(field.clone(), scalar(root, schema, field)?);
				continue;
			}

			let item = resolve(root, &schema["items"], field)?;
			if item["type"] != "object" {
				let list = match scalar(root, item, field)? {
					Scalar::String => Scalar::StringList,
					Scalar::NativePath => Scalar::NativePathList,
					_ => {
						return Err(unsupported(
							field,
							"expected an array of strings or native paths",
						));
					}
				};
				scalars.insert(field.clone(), list);
				continue;
			}

			if group.is_some() {
				return Err(unsupported(
					field,
					"only one repeated object group is supported",
				));
			}

			let mut fields = BTreeMap::new();
			for (child, schema) in object_properties(item, field)? {
				fields.insert(
					child.clone(),
					scalar(root, schema, &format!("{field}[].{child}"))?,
				);
			}
			group = Some(Group {
				field: field.clone(),
				fields,
			});
		}

		Ok(Self { scalars, group })
	}

	pub fn fields(&self) -> BTreeMap<String, Scalar> {
		let mut fields = self.scalars.clone();
		if let Some(group) = &self.group {
			for (field, scalar) in &group.fields {
				fields.insert(format!("{}[].{field}", group.field), *scalar);
			}
		}
		fields
	}
}

fn resolve<'a>(
	root: &'a Value,
	schema: &'a Value,
	field: &str,
) -> Result<&'a Value, RegistrationError> {
	resolve_bounded(root, schema, field, &mut 32)
}

fn resolve_bounded<'a>(
	root: &'a Value,
	mut schema: &'a Value,
	field: &str,
	remaining: &mut usize,
) -> Result<&'a Value, RegistrationError> {
	while *remaining > 0 {
		let Some(reference) = schema.get("$ref") else {
			return Ok(schema);
		};
		*remaining -= 1;
		let reference = reference
			.as_str()
			.and_then(|reference| reference.strip_prefix('#'))
			.ok_or_else(|| unsupported(field, "only local schema references are supported"))?;
		schema = root
			.pointer(reference)
			.ok_or_else(|| unsupported(field, "schema reference does not exist"))?;
	}

	Err(unsupported(
		field,
		"recursive schema references are unsupported",
	))
}

fn object_properties<'a>(
	schema: &'a Value,
	field: &str,
) -> Result<&'a serde_json::Map<String, Value>, RegistrationError> {
	if schema["type"] != "object"
		|| schema.get("anyOf").is_some()
		|| schema.get("oneOf").is_some()
		|| schema.get("allOf").is_some()
	{
		return Err(unsupported(field, "expected an object"));
	}
	if schema
		.get("additionalProperties")
		.is_some_and(|value| value != &Value::Bool(false))
	{
		return Err(unsupported(field, "arbitrary object keys are unsupported"));
	}

	let properties = schema["properties"]
		.as_object()
		.ok_or_else(|| unsupported(field, "expected object properties"))?;
	for name in properties.keys() {
		if name.contains("[].") {
			return Err(unsupported(
				&format!("{field}.{name}"),
				"property names cannot contain reserved group notation '[].'",
			));
		}
	}

	Ok(properties)
}

fn scalar(root: &Value, schema: &Value, field: &str) -> Result<Scalar, RegistrationError> {
	scalar_bounded(root, schema, field, &mut 32)
}

fn scalar_bounded(
	root: &Value,
	schema: &Value,
	field: &str,
	remaining: &mut usize,
) -> Result<Scalar, RegistrationError> {
	if *remaining == 0 {
		return Err(unsupported(
			field,
			"recursive or excessively deep scalar schema",
		));
	}

	*remaining -= 1;
	let schema = resolve_bounded(root, schema, field, remaining)?;
	if schema["x-trine-native-path"] == true {
		return Ok(Scalar::NativePath);
	}
	if schema.get("enum").is_some()
		|| schema.get("const").is_some()
		|| schema.get("oneOf").is_some()
		|| schema.get("allOf").is_some()
	{
		return Err(unsupported(
			field,
			"expected a string or boolean scalar, optionally nullable",
		));
	}

	if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
		if variants.len() == 2 {
			let non_null: Vec<_> = variants
				.iter()
				.filter(|value| value["type"] != "null")
				.collect();
			if non_null.len() == 1 {
				return scalar_bounded(root, non_null[0], field, remaining);
			}
		}
		return Err(unsupported(
			field,
			"only nullable scalar unions are supported",
		));
	}

	let kind = match &schema["type"] {
		Value::String(kind) => Some(kind.as_str()),
		Value::Array(kinds)
			if kinds.len() == 2 && kinds.contains(&Value::String("null".into())) =>
		{
			kinds
				.iter()
				.filter_map(Value::as_str)
				.find(|kind| *kind != "null")
		}
		_ => None,
	};

	match kind {
		Some("string") => Ok(Scalar::String),
		Some("boolean") => Ok(Scalar::Boolean),
		_ => Err(unsupported(
			field,
			"expected a string or boolean scalar, optionally nullable",
		)),
	}
}

fn unsupported(field: &str, reason: &str) -> RegistrationError {
	RegistrationError(format!(
		"unsupported schema for '{field}': {reason}; use custom CLI normalization"
	))
}
