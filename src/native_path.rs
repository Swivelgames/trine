//! Lossless host-native paths with an explicit structured transport encoding.

use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
	borrow::Cow,
	ffi::OsString,
	fmt,
	path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError(pub String);

impl fmt::Display for PathError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for PathError {}

#[derive(Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "lowercase", deny_unknown_fields)]
enum EncodedPath {
	Unix { bytes: String },
	Windows { wide: Vec<u16> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePath(PathBuf);

impl NativePath {
	pub fn new(path: impl Into<PathBuf>) -> Result<Self, PathError> {
		let path = path.into();

		#[cfg(unix)]
		let nul = {
			use std::os::unix::ffi::OsStrExt;
			path.as_os_str().as_bytes().contains(&0)
		};

		#[cfg(windows)]
		let nul = {
			use std::os::windows::ffi::OsStrExt;
			path.as_os_str().encode_wide().any(|unit| unit == 0)
		};

		if nul {
			return Err(PathError("native path contains NUL".into()));
		}

		Ok(Self(path))
	}

	pub fn as_path(&self) -> &Path {
		&self.0
	}

	pub fn into_path_buf(self) -> PathBuf {
		self.0
	}

	/// Resolve lexically against invocation context, without touching global CWD.
	pub fn resolve(&self, cwd: &Path) -> PathBuf {
		cwd.join(&self.0)
	}
}

impl AsRef<Path> for NativePath {
	fn as_ref(&self) -> &Path {
		self.as_path()
	}
}

impl Serialize for NativePath {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		if let Some(text) = self.0.to_str() {
			return serializer.serialize_str(text);
		}

		#[cfg(unix)]
		let encoded = {
			use std::os::unix::ffi::OsStrExt;
			EncodedPath::Unix {
				bytes: STANDARD.encode(self.0.as_os_str().as_bytes()),
			}
		};

		#[cfg(windows)]
		let encoded = {
			use std::os::windows::ffi::OsStrExt;
			EncodedPath::Windows {
				wide: self.0.as_os_str().encode_wide().collect(),
			}
		};

		encoded.serialize(serializer)
	}
}

impl<'de> Deserialize<'de> for NativePath {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		deserializer.deserialize_newtype_struct(NATIVE_PATH_MARKER, NativeVisitor)
	}
}

// Serde newtypes are transparent to JSON. Only our CLI deserializer recognizes
// this marker and delivers native units directly to NativeVisitor.
pub(crate) const NATIVE_PATH_MARKER: &str = "trine::NativePath";

pub(crate) fn visit_native<'de, V: serde::de::Visitor<'de>, E: serde::de::Error>(
	path: NativePath,
	visitor: V,
) -> Result<V::Value, E> {
	#[cfg(unix)]
	{
		use std::os::unix::ffi::OsStringExt;
		visitor.visit_byte_buf(path.into_path_buf().into_os_string().into_vec())
	}

	#[cfg(windows)]
	{
		use std::os::windows::ffi::OsStrExt;
		let units: Vec<u16> = path.as_path().as_os_str().encode_wide().collect();
		visitor.visit_seq(serde::de::value::SeqDeserializer::new(units.into_iter()))
	}
}

struct NativeVisitor;

impl<'de> serde::de::Visitor<'de> for NativeVisitor {
	type Value = NativePath;

	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("a native path")
	}

	fn visit_newtype_struct<D: Deserializer<'de>>(
		self,
		deserializer: D,
	) -> Result<NativePath, D::Error> {
		deserializer.deserialize_any(PathVisitor)
	}

	#[cfg(unix)]
	fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<NativePath, E> {
		use std::os::unix::ffi::OsStringExt;
		NativePath::new(OsString::from_vec(bytes)).map_err(E::custom)
	}

	#[cfg(windows)]
	fn visit_seq<S: serde::de::SeqAccess<'de>>(
		self,
		mut sequence: S,
	) -> Result<NativePath, S::Error> {
		use std::os::windows::ffi::OsStringExt;
		let mut units = Vec::new();
		while let Some(unit) = sequence.next_element::<u16>()? {
			units.push(unit);
		}

		NativePath::new(OsString::from_wide(&units)).map_err(serde::de::Error::custom)
	}
}

struct PathVisitor;

impl<'de> serde::de::Visitor<'de> for PathVisitor {
	type Value = NativePath;

	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("a Unicode path string or platform-tagged native path object")
	}

	fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<NativePath, E> {
		NativePath::new(text).map_err(E::custom)
	}

	fn visit_map<M: serde::de::MapAccess<'de>>(self, map: M) -> Result<NativePath, M::Error> {
		let encoded = EncodedPath::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
		let path = decode(encoded).map_err(serde::de::Error::custom)?;
		NativePath::new(PathBuf::from(path)).map_err(serde::de::Error::custom)
	}
}

fn decode(encoded: EncodedPath) -> Result<OsString, PathError> {
	match encoded {
		EncodedPath::Unix { bytes } => {
			let bytes = STANDARD
				.decode(bytes)
				.map_err(|error| PathError(format!("invalid Unix path base64: {error}")))?;

			#[cfg(unix)]
			{
				use std::os::unix::ffi::OsStringExt;
				Ok(OsString::from_vec(bytes))
			}

			#[cfg(windows)]
			{
				let _ = bytes;
				Err(PathError(
					"Unix path encoding is incompatible with this Windows host".into(),
				))
			}
		}
		EncodedPath::Windows { wide } => {
			#[cfg(windows)]
			{
				use std::os::windows::ffi::OsStringExt;
				Ok(OsString::from_wide(&wide))
			}

			#[cfg(unix)]
			{
				let _ = wide;
				Err(PathError(
					"Windows path encoding is incompatible with this Unix host".into(),
				))
			}
		}
	}
}

impl JsonSchema for NativePath {
	fn schema_name() -> Cow<'static, str> {
		"NativePath".into()
	}

	fn json_schema(_: &mut SchemaGenerator) -> Schema {
		schemars::json_schema!({
			"x-trine-native-path": true,
			"oneOf": [
				{"type": "string"},
				{
					"type": "object",
					"properties": {
						"platform": {"const": "unix"},
						"bytes": {"type": "string"}
					},
					"required": ["platform", "bytes"],
					"additionalProperties": false
				},
				{
					"type": "object",
					"properties": {
						"platform": {"const": "windows"},
						"wide": {
							"type": "array",
							"items": {"type": "integer", "minimum": 0, "maximum": 65535}
						}
					},
					"required": ["platform", "wide"],
					"additionalProperties": false
				}
			]
		})
	}
}
