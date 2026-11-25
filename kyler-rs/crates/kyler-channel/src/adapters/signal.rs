use std::{collections::HashSet, path::PathBuf, time::Duration};

use anyhow::{Result, ensure};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    time::timeout,
};
use uuid::Uuid;

use crate::{ChannelStore, Incoming, Settings};

pub struct Signal {
    socket: PathBuf,
    pub(crate) account: String,
    allowed: HashSet<Uuid>,
}

impl Signal {
    pub(crate) fn from_settings(settings: &Settings<'_>) -> Result<Self> {
        let socket = PathBuf::from(settings.required("KYLER_SIGNAL_SOCKET")?);
        ensure!(socket.is_absolute(), "Signal socket path must be absolute");
        let account = settings.required("KYLER_SIGNAL_ACCOUNT")?;
        ensure!(
            Uuid::parse_str(&account).is_ok()
                || account.strip_prefix('+').is_some_and(|number| {
                    (2..=15).contains(&number.len())
                        && number.bytes().all(|byte| byte.is_ascii_digit())
                }),
            "Invalid Signal account"
        );
        let allowed = settings
            .required("KYLER_SIGNAL_ALLOWED_ACIS")?
            .split(',')
            .map(|id| Uuid::parse_str(id.trim()))
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(Self {
            socket,
            account,
            allowed,
        })
    }

    pub async fn listen(&self, store: &ChannelStore) -> Result<()> {
        let stream = timeout(Duration::from_secs(10), UnixStream::connect(&self.socket)).await??;
        let mut connection = BufReader::new(stream);
        request(
            &mut connection,
            "subscribeReceive",
            json!({"account":self.account}),
        )
        .await?;
        let ack = timeout(Duration::from_secs(15), read(&mut connection)).await??;
        let subscription = ack["result"]
            .as_i64()
            .filter(|_| ack["id"] == 1)
            .ok_or_else(|| anyhow::anyhow!("Signal subscription rejected"))?;
        loop {
            let frame = read(&mut connection).await?;
            if let Some(message) = incoming(&self.account, subscription, &self.allowed, &frame) {
                store.ingest(vec![message]).await?;
            }
        }
    }

    pub(crate) async fn send(&self, chat: &str, text: &str) -> Result<(), Option<u16>> {
        let recipient = chat
            .strip_prefix("sig_")
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or(Some(400))?;
        if !self.allowed.contains(&recipient) {
            return Err(Some(403));
        }
        let result = timeout(Duration::from_secs(60), async {
            let mut connection = BufReader::new(UnixStream::connect(&self.socket).await?);
            request(
                &mut connection,
                "send",
                json!({"account":self.account,"recipient":[recipient.to_string()],"message":text}),
            )
            .await?;
            let response = read(&mut connection).await?;
            ensure!(
                response["id"] == 1
                    && response["error"].is_null()
                    && response["result"]["results"]
                        .as_array()
                        .is_some_and(
                            |results| results.len() == 1 && results[0]["type"] == "SUCCESS"
                        ),
                "Signal send failed"
            );
            Ok::<(), anyhow::Error>(())
        })
        .await;
        match result {
            Ok(Ok(())) => Ok(()),
            _ => Err(None),
        }
    }
}

async fn request(
    connection: &mut BufReader<UnixStream>,
    method: &str,
    params: Value,
) -> Result<()> {
    let mut data =
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))?;
    data.push(b'\n');
    timeout(
        Duration::from_secs(10),
        connection.get_mut().write_all(&data),
    )
    .await??;
    Ok(())
}

async fn read(connection: &mut BufReader<UnixStream>) -> Result<Value> {
    let mut line = Vec::new();
    (&mut *connection)
        .take(512 * 1024)
        .read_until(b'\n', &mut line)
        .await?;
    ensure!(
        line.last() == Some(&b'\n'),
        "Signal daemon closed the connection or sent an oversized frame"
    );
    Ok(serde_json::from_slice(&line)?)
}

fn incoming(
    account: &str,
    subscription: i64,
    allowed: &HashSet<Uuid>,
    frame: &Value,
) -> Option<Incoming> {
    if frame["method"] != "receive" || frame["params"]["subscription"] != subscription {
        return None;
    }
    let payload = &frame["params"]["result"];
    if payload.get("account").is_some_and(|value| value != account)
        || !payload["exception"].is_null()
    {
        return None;
    }
    let envelope = &payload["envelope"];
    let data = &envelope["dataMessage"];
    let sender = Uuid::parse_str(envelope["sourceUuid"].as_str()?).ok()?;
    let timestamp = data["timestamp"].as_i64()?;
    let text = data["message"].as_str()?;
    if !allowed.contains(&sender)
        || envelope["sourceNumber"] == account
        || sender.to_string() == account
        || timestamp <= 0
        || text.trim().is_empty()
        || !data["groupInfo"].is_null()
        || !data["reaction"].is_null()
        || !data["remoteDelete"].is_null()
        || data["viewOnce"] == true
    {
        return None;
    }
    Some(Incoming {
        attachments: Vec::new(),
        update: format!("sig_{sender}_{timestamp}"),
        chat: format!("sig_{sender}"),
        text: text.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_only_allowlisted_direct_signal_messages() {
        let sender = Uuid::new_v4();
        let mut event = json!({"method":"receive","params":{"subscription":3,"result":{
            "account":"+12025550100","envelope":{"sourceUuid":sender.to_string(),"dataMessage":{"message":"hello","timestamp":1}}}}});
        let allowed = HashSet::from([sender]);
        assert!(incoming("+12025550100", 3, &allowed, &event).is_some());
        assert!(incoming("+12025550100", 3, &HashSet::new(), &event).is_none());
        event["params"]["result"]["envelope"]["dataMessage"]["groupInfo"] = json!({});
        assert!(incoming("+12025550100", 3, &allowed, &event).is_none());
    }
}
