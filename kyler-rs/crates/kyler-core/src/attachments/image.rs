//! Host-side validation and normalization of user-supplied image bytes.
use std::{
    fmt::{Debug, Formatter},
    io::Cursor,
};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

use kyler_protocol::api::images::{ImageInput, MAX_IMAGE_BYTES, MAX_IMAGES};

#[derive(Clone, Serialize, Deserialize)]
pub struct UserImage {
    pub(crate) name: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) data_url: String,
}
impl UserImage {
    pub fn name(&self) -> &str {
        &self.name
    }
}
impl Debug for UserImage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserImage")
            .field("name", &self.name)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

pub fn prepare_images(inputs: &[ImageInput]) -> Result<Vec<UserImage>> {
    ensure!(
        inputs.len() <= MAX_IMAGES,
        "At most {MAX_IMAGES} images are allowed per message."
    );
    inputs
        .iter()
        .map(|input| {
            ensure!(
                input.name.len() <= 255 && !input.name.chars().any(char::is_control),
                "Invalid image name."
            );
            ensure!(
                input.data.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4,
                "Image exceeds the 5 MiB limit."
            );
            let bytes = STANDARD
                .decode(&input.data)
                .context("Invalid image Base64.")?;
            ensure!(
                bytes.len() <= MAX_IMAGE_BYTES,
                "Image exceeds the 5 MiB limit."
            );
            let mut reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
            ensure!(
                matches!(
                    reader.format(),
                    Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP)
                ),
                "Only PNG, JPEG and WebP images are supported."
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let decoded = reader.decode().context("Invalid or oversized image.")?;
            let scaled = decoded.thumbnail(2048, 2048);
            let mut png = Cursor::new(Vec::new());
            scaled.write_to(&mut png, ImageFormat::Png)?;
            ensure!(
                png.get_ref().len() <= MAX_IMAGE_BYTES,
                "Normalized image exceeds the 5 MiB limit."
            );
            Ok(UserImage {
                name: input.name.clone(),
                width: scaled.width(),
                height: scaled.height(),
                data_url: format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(png.into_inner())
                ),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::prepare_images;
    use crate::{
        ContextCompactor, ContextPolicy, Session,
        context::{ContentItem, Role, messages::build_messages},
    };

    use std::io::Cursor;

    use base64::{Engine, engine::general_purpose::STANDARD};
    use image::ImageFormat;

    use kyler_protocol::api::images::{ImageInput, MAX_IMAGE_BYTES, MAX_IMAGES};

    fn input(width: u32, height: u32) -> ImageInput {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 0, 0, 255]));
        let mut png = Cursor::new(Vec::new());
        image.write_to(&mut png, ImageFormat::Png).unwrap();
        ImageInput {
            name: "test.png".into(),
            data: STANDARD.encode(png.into_inner()),
        }
    }
    #[test]
    fn normalizes_and_bounds_image_decoding() {
        let normalized = prepare_images(&[input(4096, 1)]).unwrap();
        assert_eq!(normalized[0].width, 2048);
        assert!(normalized[0].data_url.starts_with("data:image/png;base64,"));
        assert!(prepare_images(&[input(8193, 1)]).is_err());
        let invalid = ImageInput {
            name: "bad.png".into(),
            data: "%%%".into(),
        };
        assert!(prepare_images(&[invalid.clone()]).is_err());
        assert!(prepare_images(&vec![invalid; MAX_IMAGES + 1]).is_err());
        let oversized = ImageInput {
            name: "large.png".into(),
            data: "A".repeat(MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1),
        };
        assert!(prepare_images(&[oversized]).is_err());
    }
    #[test]
    fn multimodal_projection_preserves_text_and_omits_image_bytes_from_summaries() {
        let mut session = Session::new();
        session.begin_execution("What is in this image?").unwrap();
        let image = prepare_images(&[input(1, 1)]).unwrap().remove(0);
        session.events[0].content.push(ContentItem::Image { image });
        let session: Session =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        let projected = serde_json::to_value(build_messages("system", &session).unwrap()).unwrap();
        let parts = &projected[1]["content"];
        assert_eq!(parts[0]["text"], "What is in this image?");
        assert_eq!(parts[1]["type"], "image_url");
        assert!(
            parts[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        let summary = serde_json::to_string(&session.events[0].model_view()).unwrap();
        assert!(summary.contains("test.png"));
        assert!(!summary.contains("base64"));
        let counter = ContextCompactor::new(ContextPolicy::default()).unwrap();
        let huge = serde_json::json!({"image_url":{"url":format!("data:image/png;base64,{}", "A".repeat(100_000))}});
        assert!(counter.count(&huge).unwrap() < 5000);
        assert!(counter.count(&huge).unwrap() >= 4096);
        let text = serde_json::json!({"role":"user", "content":format!("data:image/png;base64,{}", "A".repeat(100_000))});
        assert!(counter.count(&text).unwrap() > 100_000);
        assert!(matches!(
            &session.events[0].content[0],
            ContentItem::Message {
                role: Role::User,
                ..
            }
        ));
    }
}
