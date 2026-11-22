//! Request ID parsing shared by both stdio hosts. Integer IDs retain their full JSON range.

use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Id {
	String(String),
	Integer(String),
}

impl Id {
	pub fn parse(value: &Value, limit: usize) -> Option<Self> {
		match value {
			Value::String(value) if value.len() <= limit => Some(Self::String(value.clone())),
			Value::Number(value) if value.is_i64() || value.is_u64() => {
				Some(Self::Integer(value.to_string()))
			}
			_ => None,
		}
	}

	pub fn value(&self) -> Value {
		match self {
			Self::String(value) => Value::String(value.clone()),
			Self::Integer(value) => serde_json::from_str(value).expect("validated integer"),
		}
	}
}
