mod assistant;
mod bash;
mod custom;
mod developer;
mod files;
mod hook;
mod python;
mod summary;
mod tool_result;
mod user;
use std::rc::Rc;

use alvin_protocol::AgentMessage;
use gpui_kit::{AnyElement, Entity, IntoElement as _, ParentElement as _, SharedString, div};

use super::tool::ToolBlock;
pub(super) use assistant::AssistantBlock;
pub(super) use bash::input as bash_input;
pub(super) use tool_result::input as result_input;
pub(super) use user::UserBlock;

#[derive(Clone)]
pub(super) enum Block {
  User(Entity<UserBlock>),
  Assistant(Entity<AssistantBlock>),
  Tool(Entity<ToolBlock>),
  Other { id: SharedString, message: Rc<AgentMessage> },
}

impl Block {
  pub(super) fn view(&self) -> AnyElement {
    match self {
      Self::User(block) => block.clone().into_any_element(),
      Self::Assistant(block) => block.clone().into_any_element(),
      Self::Tool(block) => block.clone().into_any_element(),
      Self::Other { id, message } => match message.as_ref() {
        AgentMessage::BranchSummary(message) => summary::view(id, "Branch summary", message.summary.as_deref().unwrap_or_default()),
        AgentMessage::CompactionSummary(message) => summary::view(id, "Compaction summary", message.summary.as_deref().unwrap_or_default()),
        AgentMessage::Custom(message) => custom::view(id, message),
        AgentMessage::Developer(message) => developer::view(id, message),
        AgentMessage::PythonExecution(message) => python::view(id, message),
        AgentMessage::HookMessage(message) => hook::view(id, message),
        AgentMessage::FileMention(message) => files::view(id, message),
        AgentMessage::User(_) | AgentMessage::Assistant(_) | AgentMessage::BashExecution(_) | AgentMessage::ToolResult(_) => {
          div().child("Unexpected transcript event").into_any_element()
        }
      },
    }
  }
}
