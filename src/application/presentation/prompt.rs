//! TTY admission and styling shared by terminal decisions.
use super::CliViewContext;
use inquire::ui::{RenderConfig, Styled};
use std::io::{self, IsTerminal};

impl CliViewContext {
	pub(super) fn can_prompt(&self) -> bool {
		self.stderr.is_terminal() && io::stdin().is_terminal() && io::stderr().is_terminal()
	}
}

pub(super) fn style() -> RenderConfig<'static> {
	// Inquire adds one separator space after each prefix: three plain spaces
	// yield a four-space inset without its default question/answer ornaments.
	let mut render = RenderConfig::empty();
	render.prompt_prefix = Styled::new("   ");
	render.answered_prompt_prefix = Styled::new("   ");
	render.error_message.prefix = Styled::new("   ");
	render.canceled_prompt_indicator = Styled::new("cancelled");
	render
}
