use super::{Prompts, input::BACK};
use crate::{CliError, Result, commands::ui};

use std::collections::BTreeMap;

use kyler_channel::{Adapter, CHANNELS, Settings, WEB_ORIGIN_KEY};
use kyler_config::{AppConfig, SecretString};

pub(super) fn run(prompts: &mut Prompts, config: &mut AppConfig) -> Result<()> {
    let mut values: BTreeMap<_, _> = config
        .channels
        .iter()
        .map(|(key, value)| (key.clone(), value.expose().to_owned()))
        .collect();
    let configured = CHANNELS
        .iter()
        .any(|definition| Settings(&values).get(definition.fields[0].key).is_some());
    let skip = if configured {
        "Keep current channels"
    } else {
        "Skip for now"
    };
    if prompts.select(
        "Channels",
        vec![skip.into(), "Configure channels".into()],
        0,
    )? == 0
    {
        return Ok(());
    }
    let default_origin = if config.web.bind.ip().is_unspecified() {
        String::new()
    } else {
        kyler_config::web::connect_url(config.web.bind)
    };
    loop {
        let mut choices = vec!["Continue setup".into()];
        choices.extend(CHANNELS.iter().map(|definition| {
            if Settings(&values).get(definition.fields[0].key).is_some() {
                format!("{} · Configured", definition.name)
            } else {
                definition.name.to_owned()
            }
        }));
        let origin_choice = (Settings(&values).get(WEB_ORIGIN_KEY).is_some()
            && std::env::var_os(WEB_ORIGIN_KEY).is_none())
        .then_some(choices.len());
        if origin_choice.is_some() {
            choices.push("Edit Kyler Web origin".into());
        }
        let selected = prompts.select("Configure a channel", choices, 0)?;
        if selected == 0 {
            break;
        }
        if Some(selected) == origin_choice {
            let current = Settings(&values).get(WEB_ORIGIN_KEY).unwrap_or_default();
            let origin = prompts.channel_value("Kyler Web origin", &current, true)?;
            let mut candidate = values.clone();
            candidate.insert(WEB_ORIGIN_KEY.into(), origin);
            match Settings(&candidate).web_origin() {
                Ok(_) => values = candidate,
                Err(error) => ui::error(&error.to_string()),
            }
            continue;
        }
        let definition = &CHANNELS[selected - 1];
        let mut candidate = values.clone();
        if std::env::var_os(definition.fields[0].key).is_none()
            && candidate.contains_key(definition.fields[0].key)
        {
            match prompts.select(
                definition.name,
                vec!["Edit settings".into(), "Disable channel".into()],
                0,
            ) {
                Ok(1) => {
                    for field in definition.fields {
                        values.remove(field.key);
                    }
                    continue;
                }
                Ok(_) => {}
                Err(error) if error.code == BACK => continue,
                Err(error) => return Err(error),
            }
        }
        for field in definition.fields {
            if std::env::var_os(field.key).is_some() {
                ui::field(field.label, "Set by environment");
                continue;
            }
            let current = candidate
                .get(field.key)
                .map(String::as_str)
                .unwrap_or_default();
            let value = if field.secret {
                prompts
                    .key(field.label, &SecretString::new(current), !field.required)?
                    .expose()
                    .to_owned()
            } else {
                prompts.channel_value(field.label, current, field.required)?
            };
            if value.is_empty() {
                candidate.remove(field.key);
            } else {
                candidate.insert(field.key.to_owned(), value);
            }
        }
        if std::env::var_os(WEB_ORIGIN_KEY).is_none() && Settings(&candidate).web_origin().is_err()
        {
            let value = prompts.channel_value("Kyler Web origin", &default_origin, true)?;
            candidate.insert(WEB_ORIGIN_KEY.into(), value);
        }
        let settings = Settings(&candidate);
        if let Err(error) = settings
            .web_origin()
            .and_then(|_| Adapter::from_settings(definition, &settings))
        {
            ui::error(&format!("{}: {error}", definition.name));
            continue;
        }
        values = candidate;
    }
    config.channels = values
        .into_iter()
        .map(|(key, value)| (key, SecretString::new(value)))
        .collect();
    Ok(())
}

pub(super) fn write(config: &AppConfig, root: &mut toml::Table) {
    if config.channels.is_empty() {
        root.remove("channels");
    } else {
        let values = config
            .channels
            .iter()
            .map(|(key, value)| (key.clone(), toml::Value::from(value.expose())))
            .collect::<toml::Table>();
        root.insert("channels".into(), toml::Value::Table(values));
    }
}

pub(super) fn count(config: &AppConfig) -> usize {
    CHANNELS
        .iter()
        .filter(|definition| {
            let key = definition.fields[0].key;
            std::env::var_os(key).is_some() || config.channels.contains_key(key)
        })
        .count()
}

pub(super) fn show_configured(adapters: &[Adapter]) -> Result<()> {
    if adapters.is_empty() {
        ui::field("Channels", "None configured");
        return Ok(());
    }
    let mut details = Vec::new();
    for adapter in adapters {
        let id = adapter.name();
        let name = CHANNELS
            .iter()
            .find(|channel| channel.id == id)
            .map_or(id, |channel| channel.name);
        details.push(format!("{name}  Configured"));
        if adapter.accepts_webhook() {
            details.push(format!("  /webhooks/channels/{id}"));
        }
    }
    ui::note(
        &format!("Channels · {} configured", adapters.len()),
        &details.join("\n"),
    )
    .map_err(CliError::io)
}

pub(super) fn validate(config: &AppConfig) -> Result<Vec<Adapter>> {
    let values = config
        .channels
        .iter()
        .map(|(key, value)| (key.clone(), value.expose().to_owned()))
        .collect();
    let settings = Settings(&values);
    Adapter::configured(&settings)
        .and_then(|adapters| {
            if !adapters.is_empty() {
                settings.web_origin()?;
            }
            Ok(adapters)
        })
        .map_err(|error| CliError::new(2, format!("Invalid channel configuration: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_channels_round_trip_and_activate_the_selected_adapter() {
        let config = kyler_config::loader::parse(
            r#"
            [channels]
            KYLER_CHANNEL_WEB_URL = "http://127.0.0.1:30002"
            KYLER_TELEGRAM_BOT_TOKEN = "123:local-test"
        "#,
        )
        .unwrap();
        let mut values = toml::Table::new();
        write(&config, &mut values);
        let saved = toml::to_string(&values).unwrap();
        let restored = kyler_config::loader::parse(&saved).unwrap();
        assert!(!format!("{restored:?}").contains("123:local-test"));
        let values = restored
            .channels
            .into_iter()
            .map(|(key, value)| (key, value.expose().to_owned()))
            .collect();
        let definition = CHANNELS
            .iter()
            .find(|channel| channel.id == "telegram")
            .unwrap();
        let adapter = Adapter::from_settings(definition, &Settings(&values)).unwrap();
        assert_eq!(adapter.name(), "telegram");
        assert_eq!(adapter.namespace(), "123");
    }
}
