mod assistant;
mod bash;
mod custom;
mod job;
mod python;
mod tool_result;
mod user;
use gpui_kit::{AnyElement, Entity, IntoElement as _, ParentElement as _, SharedString, Styled as _};

use super::tool::ToolBlock;
pub(super) use assistant::AssistantBlock;
pub(super) use bash::input as bash_input;
pub(in crate::session) use custom::{SystemActivity, SystemActivityData};
pub(super) use python::input as python_input;
pub(super) use tool_result::input as result_input;
pub(super) use user::UserBlock;

#[derive(Clone)]
pub(super) enum Block {
  User(Entity<UserBlock>),
  Assistant(Entity<AssistantBlock>),
  Tool(Entity<ToolBlock>),
  Custom { id: SharedString, entity: Entity<SystemActivity> },
  Boundary(Entity<SystemActivity>),
}

impl Block {
  pub(super) fn is_timeline(&self) -> bool {
    match self {
      Self::User(_) | Self::Boundary(_) => false,
      Self::Assistant(_) | Self::Tool(_) | Self::Custom { .. } => true,
    }
  }

  pub(super) fn view(&self) -> AnyElement {
    match self {
      Self::User(block) => block.clone().into_any_element(),
      Self::Assistant(block) => block.clone().into_any_element(),
      Self::Tool(block) => block.clone().into_any_element(),
      Self::Custom { entity, .. } => entity.clone().into_any_element(),
      Self::Boundary(entity) => gpui_kit::component::v_flex()
        .w_full()
        .child(gpui_kit::component::marker::Marker::new().with_variant(gpui_kit::component::marker::MarkerVariant::Separator))
        .child(entity.clone())
        .into_any_element(),
    }
  }
}
