//! `streams config` subcommand handlers (`get`, `set`, `unset`, `path`).

use serde_json::Value;
use streams::{
    application,
    config::{ConfigKey, ConfigPath, encode_toml, toml_table_to_json, toml_value_to_json},
};

use super::{App, CliError, Output, Result, args::ConfigCommand};

impl App {
    pub(super) fn config_cmd(&self, command: ConfigCommand) -> Result<Output> {
        let format_plain = |toml_val: &toml::Value, json_val: &Value| -> Result<String> {
            Ok(match toml_val.as_table() {
                Some(table) => encode_toml(table)?.trim_end_matches('\n').to_owned(),
                None => json_val
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| json_val.to_string()),
            })
        };
        let has_catalog = || application::open_records(&self.config).is_ok();

        match command {
            ConfigCommand::Get { key, overrides } => {
                if let Some(raw_key) = key {
                    let path: ConfigPath = raw_key.parse().map_err(|_| {
                        CliError::message(format!("unknown config key {raw_key:?}"))
                    })?;
                    let toml_val = self.config.get_path_toml(path)?;
                    let json_val = toml_value_to_json(&toml_val)?;
                    let plain = format_plain(&toml_val, &json_val)?;
                    Ok(Output::plain(json_val, plain))
                } else {
                    let table = if overrides {
                        self.config.read_overrides_toml()?
                    } else {
                        self.config.values_toml()?
                    };
                    let json_val = toml_table_to_json(&table)?;
                    let text = encode_toml(&table)?;
                    Ok(Output::plain(
                        json_val,
                        text.trim_end_matches('\n').to_owned(),
                    ))
                }
            }
            ConfigCommand::Set { key, value } => {
                let parsed: ConfigKey = key
                    .parse()
                    .map_err(|_| CliError::message(format!("unknown config key {key:?}")))?;
                self.config.set(parsed, &value)?;
                let toml_val = self.config.get_path_toml(ConfigPath::Key(parsed))?;
                let json_val = toml_value_to_json(&toml_val)?;
                let plain = if parsed == ConfigKey::PiReportingModel {
                    value
                } else {
                    format_plain(&toml_val, &json_val)?
                };
                let mut output = Output::plain(json_val, plain);
                output.notify_ui = has_catalog();
                Ok(output)
            }
            ConfigCommand::Unset { key } => {
                let parsed: ConfigKey = key
                    .parse()
                    .map_err(|_| CliError::message(format!("unknown config key {key:?}")))?;
                self.config.unset(parsed)?;
                let mut output = Output::plain(Value::Null, format!("unset {key}"));
                output.notify_ui = has_catalog();
                Ok(output)
            }
            ConfigCommand::Path => {
                let path = self.config.config_path().to_string_lossy().into_owned();
                Ok(Output::plain(Value::String(path.clone()), path))
            }
        }
    }
}
