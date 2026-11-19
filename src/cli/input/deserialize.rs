//! A small Serde bridge over normalized input; native values never enter JSON.

use super::InputFields;
use crate::{NativePath, native_path};
use serde::de::{self, DeserializeSeed, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserializer, forward_to_deserialize_any};
use serde_json::{Error, Value};
use std::collections::{BTreeMap, btree_map};

pub(super) enum InputValue {
	Scalar(Value),
	Path(NativePath),
	Object(InputFields),
	Rows(Vec<InputFields>),
	Paths(Vec<NativePath>),
}

impl<'de> Deserializer<'de> for InputValue {
	type Error = Error;

	fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
		match self {
			Self::Scalar(value) => value.deserialize_any(visitor),
			Self::Object(fields) => visitor.visit_map(Fields::new(fields)?),
			Self::Rows(rows) => {
				let values = rows.into_iter().map(Self::Object);
				visitor.visit_seq(Elements {
					label: "row",
					values: values.enumerate(),
				})
			}
			Self::Paths(paths) => {
				let values = paths.into_iter().map(Self::Path);
				visitor.visit_seq(Elements {
					label: "path",
					values: values.enumerate(),
				})
			}
			Self::Path(_) => Err(de::Error::custom(
				"expected NativePath for native CLI value; buffered Serde shapes such as \
				#[serde(flatten)] cannot preserve native paths here. Use Cli::with_input \
				to extract paths with InputFields::take_path and assemble the input explicitly.",
			)),
		}
	}

	fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
		if let Self::Scalar(value) = self {
			return value.deserialize_option(visitor);
		}

		visitor.visit_some(self)
	}

	fn deserialize_newtype_struct<V: Visitor<'de>>(
		self,
		name: &'static str,
		visitor: V,
	) -> Result<V::Value, Error> {
		if name == native_path::NATIVE_PATH_MARKER
			&& let Self::Path(path) = self
		{
			return native_path::visit_native(path, visitor);
		}

		visitor.visit_newtype_struct(self)
	}

	fn deserialize_enum<V: Visitor<'de>>(
		self,
		name: &'static str,
		variants: &'static [&'static str],
		visitor: V,
	) -> Result<V::Value, Error> {
		if let Self::Scalar(value) = self {
			return value.deserialize_enum(name, variants, visitor);
		}

		self.deserialize_any(visitor)
	}

	fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
		visitor.visit_unit()
	}

	forward_to_deserialize_any! {
		bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
		bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
	}
}

struct Fields {
	entries: btree_map::IntoIter<String, InputValue>,
	pending: Option<(String, InputValue)>,
}

impl Fields {
	fn new(fields: InputFields) -> Result<Self, Error> {
		let mut values: BTreeMap<_, _> = fields
			.scalars
			.into_iter()
			.map(|(key, value)| (key, InputValue::Scalar(value)))
			.collect();
		let native = fields
			.paths
			.into_iter()
			.map(|(key, path)| (key, InputValue::Path(path)));
		let lists = fields
			.path_lists
			.into_iter()
			.map(|(key, paths)| (key, InputValue::Paths(paths)));
		let groups = fields
			.groups
			.into_iter()
			.map(|(key, rows)| (key, InputValue::Rows(rows)));

		for (key, value) in native.chain(lists).chain(groups) {
			if values.contains_key(&key) {
				return Err(de::Error::custom(format!(
					"field `{key}` has multiple input representations"
				)));
			}
			values.insert(key, value);
		}

		Ok(Self {
			entries: values.into_iter(),
			pending: None,
		})
	}
}

impl<'de> MapAccess<'de> for Fields {
	type Error = Error;

	fn next_key_seed<K: DeserializeSeed<'de>>(
		&mut self,
		seed: K,
	) -> Result<Option<K::Value>, Error> {
		let Some((key, value)) = self.entries.next() else {
			return Ok(None);
		};
		self.pending = Some((key.clone(), value));
		seed.deserialize(key.into_deserializer()).map(Some)
	}

	fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
		let (key, value) = self
			.pending
			.take()
			.ok_or_else(|| de::Error::custom("value requested without field"))?;

		seed.deserialize(value)
			.map_err(|error| de::Error::custom(format!("field `{key}`: {error}")))
	}
}

struct Elements<I> {
	label: &'static str,
	values: std::iter::Enumerate<I>,
}

impl<'de, I: Iterator<Item = InputValue>> SeqAccess<'de> for Elements<I> {
	type Error = Error;

	fn next_element_seed<T: DeserializeSeed<'de>>(
		&mut self,
		seed: T,
	) -> Result<Option<T::Value>, Error> {
		let Some((index, value)) = self.values.next() else {
			return Ok(None);
		};

		seed.deserialize(value)
			.map(Some)
			.map_err(|error| de::Error::custom(format!("{} [{index}]: {error}", self.label)))
	}
}
