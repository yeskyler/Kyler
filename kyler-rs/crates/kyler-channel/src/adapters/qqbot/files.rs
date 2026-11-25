use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use url::Url;

use super::QqBot;
use crate::{IncomingAttachment, ReplyReceipt};

/// Upload tokens stay server-side and are cached no longer than QQ's TTL.
pub struct UploadedFile {
    pub file_info: String,
    pub expires_at: i64,
}
fn uploaded_file(value: &Value) -> Result<UploadedFile, Option<u16>> {
    let file_info = value["file_info"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(Some(502))?
        .to_owned();
    let ttl = value["ttl"].as_u64().unwrap_or_default().min(300);
    Ok(UploadedFile {
        file_info,
        expires_at: chrono::Utc::now().timestamp() + ttl.saturating_sub(5) as i64,
    })
}

pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;

fn attachment_url(value: &str) -> Result<Url> {
    let value = if value.starts_with("//") {
        format!("https:{value}")
    } else {
        value.to_owned()
    };
    let mut url = Url::parse(&value).map_err(|_| anyhow::anyhow!("Invalid QQ attachment URL"))?;
    // QQ sometimes supplies HTTP CDN URLs; always download over TLS.
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Invalid QQ attachment scheme"
    );
    url.set_scheme("https")
        .map_err(|_| anyhow::anyhow!("Invalid QQ attachment scheme"))?;
    let host = url.host_str().unwrap_or_default();
    ensure!(
        ["qq.com", "qpic.cn", "gtimg.cn"]
            .iter()
            .any(|domain| host.ends_with(&format!(".{domain}"))),
        "Unsupported QQ attachment host"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443),
        "Invalid QQ attachment authority"
    );
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || ip.is_documentation()
                || (ip.octets()[0] == 198 && (18..=19).contains(&ip.octets()[1]))
                || (ip.octets()[0] == 192 && ip.octets()[1] == 0 && ip.octets()[2] == 0)
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 240
                || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1])))
        }
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped().is_none()
                && (ip.segments()[0] & 0xe000) == 0x2000
                && ip.segments()[0..2] != [0x2001, 0xdb8]
        }
    }
}

/// Resolve once and pin the public CDN addresses to prevent DNS rebinding.
pub async fn download(file: &IncomingAttachment) -> Result<Vec<u8>> {
    ensure!(
        file.size.is_none_or(|n| n <= MAX_FILE_BYTES as u64),
        "QQ attachment exceeds 20 MiB"
    );
    let url = attachment_url(&file.url)?;
    let host = url.host_str().unwrap();
    let addresses: Vec<SocketAddr> =
        tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, 443)))
            .await
            .map_err(|_| anyhow::anyhow!("QQ attachment DNS timed out"))?
            .map_err(|_| anyhow::anyhow!("QQ attachment DNS failed"))?
            .collect();
    ensure!(
        !addresses.is_empty() && addresses.iter().all(|a| public_ip(a.ip())),
        "QQ attachment address is not public"
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(60))
        .resolve_to_addrs(host, &addresses)
        .build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("QQ attachment download failed"))?;
    ensure!(
        response.status().is_success(),
        "QQ attachment download was rejected"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MAX_FILE_BYTES as u64),
        "QQ attachment exceeds 20 MiB"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("QQ attachment transfer failed"))?
    {
        ensure!(
            bytes.len() + chunk.len() <= MAX_FILE_BYTES,
            "QQ attachment exceeds 20 MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    ensure!(!bytes.is_empty(), "QQ attachment is empty");
    Ok(bytes)
}

impl QqBot {
    pub async fn upload_file(
        &self,
        chat: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<UploadedFile, Option<u16>> {
        let openid = super::chat_openid(chat)?;
        if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
            return Err(Some(413));
        }
        if bytes.len() > 8 * 1024 * 1024 {
            return tokio::time::timeout(
                Duration::from_secs(180),
                self.upload_parts(openid, name, mime, bytes),
            )
            .await
            .map_err(|_| None)?;
        }
        let token = self.token().await?;
        let response = self
            .http
            .post(self.api_endpoint(openid, "files"))
            .header("authorization", format!("QQBot {token}"))
            .timeout(Duration::from_secs(90))
            .json(&upload_body(name, mime, bytes))
            .send()
            .await
            .map_err(|_| None)?;
        let status = response.status();
        if status.as_u16() == 401 {
            *self.access.lock().await = None;
        }
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        let value: Value = response.json().await.map_err(|_| None)?;
        uploaded_file(&value)
    }

    async fn upload_api(
        &self,
        openid: &str,
        action: &str,
        body: &Value,
    ) -> Result<Value, Option<u16>> {
        let token = self.token().await?;
        let response = self
            .http
            .post(self.api_endpoint(openid, action))
            .header("authorization", format!("QQBot {token}"))
            .timeout(Duration::from_secs(90))
            .json(body)
            .send()
            .await
            .map_err(|_| None)?;
        let status = response.status();
        if status.as_u16() == 401 {
            *self.access.lock().await = None;
        }
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        if status == reqwest::StatusCode::NO_CONTENT {
            return Ok(json!({}));
        }
        let value: Value = response.json().await.map_err(|_| None)?;
        if let Some(code) = value["code"].as_i64().filter(|n| *n != 0) {
            return Err(Some(if code == 40093001 { 503 } else { 400 }));
        }
        Ok(value)
    }

    async fn upload_parts(
        &self,
        openid: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<UploadedFile, Option<u16>> {
        let prepared = self
            .upload_api(openid, "upload_prepare", &prepare_body(name, mime, bytes))
            .await?;
        let upload_id = prepared["upload_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(Some(502))?;
        let parts = part_ranges(&prepared, bytes.len()).map_err(|_| Some(502))?;
        // Bounded serial transfer avoids holding multiple 20 MiB buffers or
        // exceeding the server's concurrency limit on small deployments.
        for (index, start, end, url) in parts {
            let url = Url::parse(&url).map_err(|_| Some(502))?;
            let host = url.host_str().unwrap_or_default();
            if url.scheme() != "https"
                || !host.ends_with(".myqcloud.com")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port_or_known_default() != Some(443)
            {
                return Err(Some(502));
            }
            let addresses: Vec<_> =
                tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, 443)))
                    .await
                    .map_err(|_| None)?
                    .map_err(|_| None)?
                    .collect();
            if addresses.is_empty() || !addresses.iter().all(|a| public_ip(a.ip())) {
                return Err(Some(502));
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(host, &addresses)
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(90))
                .build()
                .map_err(|_| None)?;
            let response = client
                .put(url)
                .body(bytes[start..end].to_vec())
                .send()
                .await
                .map_err(|_| None)?;
            if !response.status().is_success() {
                return Err(Some(response.status().as_u16()));
            }
            use md5::Digest;
            self.upload_api(openid,"upload_part_finish",&json!({"upload_id":upload_id,"part_index":index,"block_size":end-start,"md5":format!("{:x}",md5::Md5::digest(&bytes[start..end]))})).await?;
        }
        let completed = self
            .upload_api(openid, "files", &json!({"upload_id":upload_id}))
            .await?;
        uploaded_file(&completed)
    }

    pub async fn send_file(
        &self,
        chat: &str,
        file_info: &str,
        receipt: Option<&ReplyReceipt>,
    ) -> Result<(), Option<u16>> {
        let openid = super::chat_openid(chat)?;
        let receipt = super::active_receipt(receipt)?;
        let token = self.token().await?;
        let response = self.http.post(self.api_endpoint(openid, "messages"))
            .header("authorization", format!("QQBot {token}"))
            .json(&json!({"msg_type":7,"media":{"file_info":file_info},"msg_id":receipt.message_id.strip_prefix("qq_").ok_or(Some(400))?,"msg_seq":receipt.sequence}))
            .send().await.map_err(|_| None)?;
        let status = response.status();
        if status.as_u16() == 401 {
            *self.access.lock().await = None;
        }
        if !status.is_success() {
            return Err(Some(status.as_u16()));
        }
        let value: Value = response.json().await.map_err(|_| None)?;
        if value["code"].as_i64().is_some_and(|code| code != 0) {
            return Err(Some(400));
        }
        Ok(())
    }
}

fn prepare_body(name: &str, mime: &str, bytes: &[u8]) -> Value {
    use md5::Digest;
    use sha1::Digest as _;
    json!({"file_type":if matches!(mime,"image/png" | "image/jpeg") {1} else {4},
        "file_name":name,"file_size":bytes.len(),
        "md5":format!("{:x}",md5::Md5::digest(bytes)),
        "sha1":hex::encode(sha1::Sha1::digest(bytes)),
        "md5_10m":format!("{:x}",md5::Md5::digest(&bytes[..bytes.len().min(10_002_432)]))})
}

/// Validate exact coverage before uploading any bytes to presigned URLs.
fn part_ranges(prepared: &Value, size: usize) -> Result<Vec<(u64, usize, usize, String)>> {
    let block = prepared["block_size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX_FILE_BYTES as u64)
        .ok_or_else(|| anyhow::anyhow!("Invalid QQ upload block"))? as usize;
    let parts = prepared["parts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing QQ upload parts"))?;
    ensure!(
        !parts.is_empty() && parts.len() <= 128,
        "Invalid QQ upload part count"
    );
    let mut ranges = Vec::new();
    for part in parts {
        let index = part["index"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= 128)
            .ok_or_else(|| anyhow::anyhow!("Invalid QQ part index"))?;
        let start = (index as usize - 1) * block;
        ensure!(start < size, "Invalid QQ part offset");
        let length = part["block_size"]
            .as_u64()
            .filter(|n| *n > 0)
            .unwrap_or(block as u64)
            .min((size - start) as u64) as usize;
        let url = part["presigned_url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing QQ upload URL"))?;
        ranges.push((index, start, start + length, url.to_owned()));
    }
    ranges.sort_by_key(|p| p.1);
    let mut end = 0;
    for (_, start, next, _) in &ranges {
        ensure!(*start == end, "Invalid QQ upload coverage");
        end = *next;
    }
    ensure!(end == size, "Incomplete QQ upload coverage");
    Ok(ranges)
}

fn upload_body(name: &str, mime: &str, bytes: &[u8]) -> Value {
    json!({"file_type":if matches!(mime, "image/png" | "image/jpeg") {1} else {4},
        "file_name":name,"file_data":STANDARD.encode(bytes),"srv_send_msg":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_cache_never_outlives_platform_ttl() {
        let now = chrono::Utc::now().timestamp();
        let short = uploaded_file(&json!({"file_info":"token","ttl":30})).unwrap();
        assert!((now + 24..=now + 26).contains(&short.expires_at));
        let unknown = uploaded_file(&json!({"file_info":"token"})).unwrap();
        assert!(unknown.expires_at <= chrono::Utc::now().timestamp());
        assert!(uploaded_file(&json!({"ttl":300})).is_err());
    }

    #[test]
    fn only_public_qq_cdn_urls_are_allowed() {
        assert_eq!(
            attachment_url("http://multimedia.nt.qq.com/a?key=private")
                .unwrap()
                .scheme(),
            "https"
        );
        for value in [
            "https://qq.com.evil.test/a",
            "https://127.0.0.1/a",
            "file:///etc/passwd",
            "https://user@a.qpic.cn/a",
            "https://a.qpic.cn:444/a",
        ] {
            assert!(attachment_url(value).is_err());
        }
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()));
        }
        assert!(public_ip("1.1.1.1".parse().unwrap()));
    }
    #[test]
    fn upload_parts_require_exact_coverage_and_valid_hashes() {
        let body = prepare_body("file.txt", "text/plain", b"abc");
        assert_eq!(body["md5"], "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(body["sha1"], "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(body["md5"], body["md5_10m"]);
        let mut prepared = json!({"block_size":2,"parts":[{"index":1,"presigned_url":"https://bucket.myqcloud.com/1"},{"index":2,"presigned_url":"https://bucket.myqcloud.com/2"}]});
        let ranges = part_ranges(&prepared, 3).unwrap();
        assert_eq!((ranges[0].1, ranges[0].2), (0, 2));
        assert_eq!((ranges[1].1, ranges[1].2), (2, 3));
        prepared["parts"][1]["index"] = json!(1);
        assert!(part_ranges(&prepared, 3).is_err());
        prepared["parts"] = json!([]);
        assert!(part_ranges(&prepared, 3).is_err());
    }

    #[test]
    fn uploads_binary_without_public_urls_or_automatic_messages() {
        let body = upload_body("report.pdf", "application/pdf", b"fixture");
        assert_eq!(body["file_type"], 4);
        assert_eq!(body["file_data"], "Zml4dHVyZQ==");
        assert_eq!(body["srv_send_msg"], false);
        assert!(body.get("url").is_none());
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn local_binary_upload_and_media_message_preserve_reply_identity() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for stage in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(index) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]).to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|s| s.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                while request.len() < header_end + length {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let body: Value =
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
                assert!(headers.contains("authorization: qqbot test_only_token"));
                let response = if stage == 0 {
                    assert!(headers.starts_with("post /v2/users/user/files "));
                    assert_eq!(body["file_name"], "report.pdf");
                    assert_eq!(body["file_data"], STANDARD.encode(b"%PDF-fixture"));
                    assert_eq!(body["srv_send_msg"], false);
                    r#"{"file_info":"opaque-file-token","ttl":300}"#
                } else {
                    assert!(headers.starts_with("post /v2/users/user/messages "));
                    assert_eq!(body["msg_type"], 7);
                    assert_eq!(body["media"]["file_info"], "opaque-file-token");
                    assert_eq!(body["msg_id"], "incoming-id");
                    assert_eq!(body["msg_seq"], 2);
                    r#"{"id":"sent-id"}"#
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).as_bytes()).await.unwrap();
            }
        });
        let mut values = std::collections::BTreeMap::new();
        values.insert("KYLER_QQBOT_APP_ID".into(), "123456".into());
        values.insert("KYLER_QQBOT_APP_SECRET".into(), "TEST_ONLY_SECRET".into());
        let mut bot = QqBot::from_settings(&crate::Settings(&values)).unwrap();
        bot.api_base = Some(format!("http://{address}"));
        bot.http = reqwest::Client::builder().no_proxy().build().unwrap();
        *bot.access.lock().await = Some((
            "TEST_ONLY_TOKEN".into(),
            std::time::Instant::now() + Duration::from_secs(600),
        ));
        let token = bot
            .upload_file("qq_user", "report.pdf", "application/pdf", b"%PDF-fixture")
            .await
            .unwrap();
        let mut receipt = ReplyReceipt {
            message_id: "qq_incoming-id".into(),
            sequence: 2,
            expires: chrono::Utc::now().timestamp() + 600,
            context_token: None,
        };
        bot.send_file("qq_user", &token.file_info, Some(&receipt))
            .await
            .unwrap();
        receipt.expires = 0;
        assert_eq!(
            bot.send_file("qq_user", &token.file_info, Some(&receipt))
                .await,
            Err(Some(403))
        );
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
