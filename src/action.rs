//! Typed action registration without transport or executor types.

use std::{fmt, future::Future, pin::Pin, sync::Arc};

use schemars::JsonSchema;

use crate::InvocationContext;

type Handler<Input, Output, Error, Event> = dyn Fn(
		Input,
		InvocationContext<Event>,
	) -> Pin<Box<dyn Future<Output = Result<Output, Error>> + Send>>
	+ Send
	+ Sync;

/// A registration or CLI binding cannot be represented by this slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationError(pub String);

impl fmt::Display for RegistrationError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

impl std::error::Error for RegistrationError {}

struct CloseEvents<Event>(InvocationContext<Event>);

impl<Event> Drop for CloseEvents<Event> {
	fn drop(&mut self) {
		self.0.close_events();
	}
}

/// Register a typed async handler once, then invoke it directly or through a view.
pub struct Action<Input, Output, Error, Event> {
	name: String,
	description: String,
	schema: serde_json::Value,
	handler: Arc<Handler<Input, Output, Error, Event>>,
}

impl<Input, Output, Error, Event> Clone for Action<Input, Output, Error, Event> {
	fn clone(&self) -> Self {
		Self {
			name: self.name.clone(),
			description: self.description.clone(),
			schema: self.schema.clone(),
			handler: self.handler.clone(),
		}
	}
}

impl<Input, Output, Error, Event> Action<Input, Output, Error, Event> {
	pub fn new<HandlerFn, HandlerFuture>(
		name: impl Into<String>,
		description: impl Into<String>,
		handler: HandlerFn,
	) -> Result<Self, RegistrationError>
	where
		Input: JsonSchema,
		HandlerFn: Fn(Input, InvocationContext<Event>) -> HandlerFuture + Send + Sync + 'static,
		HandlerFuture: Future<Output = Result<Output, Error>> + Send + 'static,
	{
		let name = name.into();
		validate_name(&name)?;

		let description = description.into();
		if description.trim().is_empty() {
			return Err(RegistrationError(format!(
				"action '{name}' requires a description"
			)));
		}

		Ok(Self {
			name,
			description,
			schema: schemars::schema_for!(Input).to_value(),
			handler: Arc::new(move |input, context| Box::pin(handler(input, context))),
		})
	}

	pub fn name(&self) -> &str {
		&self.name
	}

	pub fn description(&self) -> &str {
		&self.description
	}

	pub fn input_schema(&self) -> &serde_json::Value {
		&self.schema
	}

	/// Completion (or dropping this future) closes progress from all context
	/// clones. Cancellation alone does not drop the handler: it must cooperate.
	pub fn invoke(
		&self,
		input: Input,
		context: InvocationContext<Event>,
	) -> impl Future<Output = Result<Output, Error>> {
		let completion = CloseEvents(context.clone());

		async move {
			let _completion = completion;
			(self.handler)(input, context).await
		}
	}
}

pub(crate) fn validate_name(name: &str) -> Result<(), RegistrationError> {
	if name
		.as_bytes()
		.first()
		.is_none_or(|byte| !byte.is_ascii_lowercase())
		|| !name
			.bytes()
			.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
	{
		return Err(RegistrationError(format!(
			"invalid name '{name}': use a lowercase letter followed by lowercase letters, digits or '-'"
		)));
	}

	Ok(())
}
