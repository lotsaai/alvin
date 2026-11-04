use std::sync::Arc;

use alvin_protocol::ImageContent;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
  AnyElement, Image, ImageFormat, ImageSource, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
  StatefulInteractiveElement as _, Styled as _, StyledImage as _, div, img, rems,
};

pub(in crate::session::transcript) struct PreparedImage {
  original: ImageContent,
  source: crate::Result<ImageSource>,
}

impl PreparedImage {
  pub(in crate::session::transcript) fn new(image: &ImageContent) -> Self {
    let source = if let Some(data) = &image.data {
      match image.mime_type.as_deref() {
        Some(mime) => match ImageFormat::from_mime_type(mime) {
          Some(format) => STANDARD
            .decode(data)
            .map(|bytes| ImageSource::Image(Arc::new(Image::from_bytes(format, bytes))))
            .map_err(|source| crate::Error::ImageData { source }),
          None => Err(crate::Error::ImageMimeUnsupported { mime: mime.to_owned() }),
        },
        None => Err(crate::Error::ImageMimeMissing),
      }
    } else if let Some(url) = &image.url {
      Ok(ImageSource::from(url.clone()))
    } else {
      Err(crate::Error::ImageSourceMissing)
    };
    Self {
      original: image.clone(),
      source,
    }
  }

  pub(in crate::session::transcript) fn sync(&mut self, image: &ImageContent) {
    if self.original != *image {
      *self = Self::new(image);
    }
  }

  pub(in crate::session::transcript) fn view(&self, id: SharedString) -> AnyElement {
    let label = format!("Image ({})", self.original.mime_type.as_deref().unwrap_or("unknown format"));
    let content = match &self.source {
      Ok(source) => div()
        .id(id)
        .test_support()
        .role(gpui_kit::Role::Image)
        .aria_label(SharedString::from(label))
        .child(
          img(source.clone())
            .object_fit(gpui_kit::ObjectFit::Contain)
            .with_fallback(|| {
              div()
                .child("Image could not be displayed. Inspect raw data for details.")
                .into_any_element()
            })
            .id("image-content")
            .test_support()
            .h(rems(14.))
            .max_w_full()
            .min_w_0()
            .min_h_0(),
        )
        .into_any_element(),
      Err(error) => div()
        .id(id)
        .test_support()
        .child(SharedString::from(error.to_string()))
        .into_any_element(),
    };
    div().w_full().max_w(rems(28.)).min_w_0().child(content).into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::ImageContent;
  use gpui_kit::{ImageFormat, ImageSource};

  use super::PreparedImage;

  #[test]
  fn captured_image_is_prepared_as_real_webp_bytes() {
    let events: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_image_url_read.events.json")).unwrap();
    let image = events
      .as_array()
      .unwrap()
      .iter()
      .filter_map(|event| event.pointer("/body/frame"))
      .find(|frame| frame["type"] == "tool_execution_end" && frame["toolName"] == "read")
      .unwrap()["result"]["content"]
      .as_array()
      .unwrap()
      .iter()
      .find(|block| block["type"] == "image")
      .unwrap();
    let image: ImageContent = serde_json::from_value(image.clone()).unwrap();
    let mut prepared = PreparedImage::new(&image);
    let ImageSource::Image(source) = prepared.source.as_ref().unwrap() else {
      panic!("expected image bytes")
    };
    assert_eq!(source.format, ImageFormat::Webp);
    assert!(source.bytes.starts_with(b"RIFF"));
    let original = source.clone();
    prepared.sync(&image);
    let ImageSource::Image(source) = prepared.source.as_ref().unwrap() else {
      panic!("expected image bytes")
    };
    assert!(std::sync::Arc::ptr_eq(&original, source));
  }

  #[test]
  fn invalid_data_and_unsupported_mime_are_visible_errors() {
    let image = ImageContent {
      data: Some("not base64!".into()),
      mime_type: Some("image/png".into()),
      ..Default::default()
    };
    let error = PreparedImage::new(&image).source.err().unwrap();
    assert!(matches!(error, crate::Error::ImageData { .. }));
    assert!(std::error::Error::source(&error).is_some());
    let image = ImageContent {
      mime_type: Some("image/unknown".into()),
      ..image
    };
    assert!(matches!(
      PreparedImage::new(&image).source,
      Err(crate::Error::ImageMimeUnsupported { .. })
    ));
    let image = ImageContent { mime_type: None, ..image };
    assert!(matches!(PreparedImage::new(&image).source, Err(crate::Error::ImageMimeMissing)));
    let image = ImageContent { data: None, ..image };
    assert!(matches!(PreparedImage::new(&image).source, Err(crate::Error::ImageSourceMissing)));
  }
}
