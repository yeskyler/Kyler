//! Session skill catalog, approval and resource requests.
use super::{Client, valid_id};
use crate::error::{Error, Result};

use reqwest::Method;

use kyler_protocol::api::skills::{SkillAction, SkillActionRequest, SkillDetails, SkillInfo};

impl Client {
    pub async fn user_skills(&self) -> Result<Vec<SkillInfo>> {
        self.json(self.request(Method::POST, "/v1/plugins/skills/reload"))
            .await
    }
    pub async fn user_skill_details(&self, id: &str) -> Result<SkillDetails> {
        skill_id(id)?;
        self.json(self.request(Method::GET, &format!("/v1/plugins/skills/{id}")))
            .await
    }
    pub async fn install_user_skill(&self, archive: Vec<u8>) -> Result<SkillInfo> {
        if archive.len() > kyler_protocol::api::skills::MAX_SKILL_ARCHIVE_BYTES {
            return Err(Error::invalid("Skill archive exceeds 20 MiB."));
        }
        self.json(
            self.request(Method::POST, "/v1/plugins/skills/upload")
                .header("content-type", "application/zip")
                .body(archive),
        )
        .await
    }
    pub async fn remove_user_skill(&self, id: &str, fingerprint: String) -> Result<()> {
        skill_id(id)?;
        let response = self
            .request(Method::DELETE, &format!("/v1/plugins/skills/{id}"))
            .json(&SkillActionRequest { fingerprint })
            .send()
            .await
            .map_err(|_| Error::transport())?;
        self.checked(response).await?;
        Ok(())
    }

    pub async fn skills(&self, session: &str, reload: bool) -> Result<Vec<SkillInfo>> {
        valid_id(session)?;
        let (method, suffix) = if reload {
            (Method::POST, "/reload")
        } else {
            (Method::GET, "")
        };
        self.json(self.request(method, &format!("/v1/sessions/{session}/skills{suffix}")))
            .await
    }
    pub async fn skill_details(&self, session: &str, skill: &str) -> Result<SkillDetails> {
        valid_id(session)?;
        skill_id(skill)?;
        self.json(self.request(
            Method::GET,
            &format!("/v1/sessions/{session}/skills/{skill}"),
        ))
        .await
    }
    pub async fn skill_action(
        &self,
        session: &str,
        skill: &str,
        action: SkillAction,
        fingerprint: String,
    ) -> Result<()> {
        valid_id(session)?;
        skill_id(skill)?;
        let action = match action {
            SkillAction::Enable => "enable",
            SkillAction::Disable => "disable",
        };
        let response = self
            .request(
                Method::POST,
                &format!("/v1/sessions/{session}/skills/{skill}/{action}"),
            )
            .json(&SkillActionRequest { fingerprint })
            .send()
            .await
            .map_err(|_| Error::transport())?;
        self.checked(response).await?;
        Ok(())
    }
}

fn skill_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::invalid("Invalid skill ID."));
    }
    Ok(())
}
