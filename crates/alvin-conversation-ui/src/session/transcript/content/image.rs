use alvin_protocol::ImageContent;
use gpui_kit::component::attachment::{Attachment, AttachmentContent, AttachmentDescription, AttachmentMedia, AttachmentTitle};
use gpui_kit::{AnyElement, IntoElement as _};

pub(in crate::session::transcript) fn attachment(image: &ImageContent) -> AnyElement {
  attachment_parts(
    image.mime_type.as_deref(),
    image.data.as_ref().map(String::len),
    image.url.as_deref(),
  )
}

pub(in crate::session::transcript) fn attachment_parts(mime: Option<&str>, size: Option<usize>, url: Option<&str>) -> AnyElement {
  let description = match size {
    Some(size) => format!("{} · {size} encoded bytes", mime.unwrap_or("unknown format")),
    None => mime.unwrap_or("unknown format").to_owned(),
  };
  let mut media = AttachmentMedia::new();
  if let Some(url) = url {
    media = media.src(url.to_owned());
  }
  Attachment::new()
    .media(media)
    .content(
      AttachmentContent::new()
        .title(AttachmentTitle::new("Image"))
        .description(AttachmentDescription::new(description)),
    )
    .into_any_element()
}
