//! One user turn, independent of transport or frontend.
use crate::attachments::image::UserImage;

use kyler_protocol::api::attachments::AttachmentInfo;

#[derive(Default)]
pub struct UserTurn {
    pub text: String,
    pub skills: Vec<kyler_protocol::api::skills::SkillReference>,
    /// Compatibility input; new clients use attachment references.
    pub images: Vec<UserImage>,
    pub attachments: Vec<AttachmentInfo>,
    pub resume: bool,
    /// Compact history without running a normal model/tool turn.
    pub compact: bool,
}
