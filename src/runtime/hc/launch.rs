//! Prepared hc terminal creation. Applying it never selects work or retries a start.
use super::{HcOperation, HcPresentation, HcWorkExecution};
use crate::{
    frontend::{CommandEvidence, CommandOutcome, FrontendId, NativeRequest, hc::HcEndpoint},
    integration::{AdoptionEffect, ConversationId, Effect, LaunchItem, NativeRef, WorkScope},
    runtime::{Result, launch::resolve_executable, workspace::native_environment},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Owner-local inputs needed to prepare one work terminal.
pub struct StartContext<'a> {
    pub scope: &'a WorkScope,
    pub stream: &'a str,
    pub space: &'a NativeRef,
    pub socket: &'a HcEndpoint,
    pub environment: &'a BTreeMap<String, String>,
}

/// Exact start parameters prepared before any terminal effect.
pub struct PreparedTerminal {
    endpoint: crate::frontend::EndpointId,
    socket: std::path::PathBuf,
    tab: String,
    params: Value,
}
impl PreparedTerminal {
    pub fn prepare(item: &LaunchItem, context: StartContext<'_>) -> Result<Self> {
        context
            .scope
            .validate()
            .map_err(|error| crate::runtime::invalid(error.to_string()))?;
        crate::runtime::text(context.stream)?;
        context
            .space
            .validate()
            .map_err(|error| crate::runtime::invalid(error.to_string()))?;
        if context.space.adapter != FrontendId::Hc || context.space.key.contains('/') {
            return Err(crate::runtime::invalid("invalid hc placement reference"));
        }
        let (cwd, mut environment, argv) = match item {
            LaunchItem::Conversation(launch) => {
                if launch.conversation().harness != "pi" {
                    return Err(crate::runtime::invalid("hc work opening supports Pi only"));
                }
                let command = launch.command().argv();
                let executable =
                    resolve_executable(&command[0], launch.environment(), launch.cwd())?;
                let program = executable.to_str().ok_or_else(|| {
                    crate::runtime::invalid("owner executable path must be UTF-8")
                })?;
                let argv: Vec<_> = std::iter::once(program.to_owned())
                    .chain(command[1..].iter().cloned())
                    .collect();
                (launch.cwd(), launch.environment().clone(), Some(argv))
            }
            LaunchItem::Shell { cwd } => (
                cwd.as_path(),
                native_environment(context.stream, context.environment)?,
                None,
            ),
        };
        if !cwd.is_absolute() || cwd.to_str().is_none() {
            return Err(crate::runtime::invalid(
                "hc terminal directory must be an absolute UTF-8 path",
            ));
        }
        if environment.get("STREAMS_STREAM_ID").map(String::as_str) != Some(context.stream) {
            return Err(crate::runtime::invalid(
                "hc launch Stream does not match its routing",
            ));
        }
        if environment
            .keys()
            .any(|key| key.starts_with("HC_") && key != "HC_SOCKET" && key != "HC_SESSION")
        {
            return Err(crate::runtime::invalid(
                "caller terminal identity cannot be forwarded",
            ));
        }
        let socket = context
            .socket
            .socket()
            .to_str()
            .ok_or_else(|| crate::runtime::invalid("hc socket path must be UTF-8"))?;
        for (key, value) in [
            ("HC_SOCKET", Some(socket)),
            (
                "HC_SESSION",
                context.environment.get("HC_SESSION").map(String::as_str),
            ),
            (
                "XDG_RUNTIME_DIR",
                context
                    .environment
                    .get("XDG_RUNTIME_DIR")
                    .map(String::as_str),
            ),
        ] {
            if let Some(value) = value {
                if environment
                    .get(key)
                    .is_some_and(|existing| existing != value)
                {
                    return Err(crate::runtime::invalid("conflicting hc launch selector"));
                }
                environment.insert(key.into(), value.into());
            }
        }
        if environment
            .iter()
            .any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0'))
        {
            return Err(crate::runtime::invalid("invalid hc launch environment"));
        }
        let tab = uuid::Uuid::new_v4().to_string();
        let routing = serde_json::to_string(
            &json!({"version":1,"scope":context.scope,"stream":context.stream}),
        )
        .expect("routing label contains JSON values");
        let mut params = json!({"cwd":cwd,"env":environment,"labels":{"streams.work":routing,"herdr.space":context.space.key,"herdr.tab":tab},"no_parent":true,"wait":false});
        if let Some(argv) = argv {
            params["argv"] = json!(argv);
        }
        Ok(Self {
            endpoint: context.space.endpoint.clone(),
            socket: context.socket.socket().to_path_buf(),
            tab,
            params,
        })
    }
    pub fn tab(&self) -> &str {
        &self.tab
    }
}

/// One selected, verified and unplaced terminal with fixed placement labels.
pub struct PreparedAdoption {
    terminal: NativeRef,
    socket: std::path::PathBuf,
    tab: String,
    params: Value,
}
impl PreparedAdoption {
    pub fn tab(&self) -> &str {
        &self.tab
    }
    pub fn terminal(&self) -> &NativeRef {
        &self.terminal
    }
}
impl HcWorkExecution {
    pub fn prepare_adoptions(
        &self,
        conversations: &[ConversationId],
        presentation: &HcPresentation,
        space: &NativeRef,
        socket: &HcEndpoint,
    ) -> std::result::Result<Vec<PreparedAdoption>, crate::frontend::FrontendError> {
        space
            .validate()
            .map_err(|_| super::invalid("hc placement is unavailable"))?;
        if space.adapter != FrontendId::Hc
            || space.endpoint != self.sources.endpoint
            || space.key.contains('/')
        {
            return Err(super::invalid("hc placement belongs to another core"));
        }
        Ok(self.unplaced_selected(conversations, presentation)?.into_iter().map(|terminal| {
            let tab = uuid::Uuid::new_v4().to_string();
            let params = json!({"terminal":terminal.key,"labels":{"herdr.space":space.key,"herdr.tab":tab}});
            PreparedAdoption { terminal, socket:socket.socket().to_path_buf(), tab, params }
        }).collect())
    }
}

impl HcOperation {
    /// Submit one prepared adoption once; never replay an unconfirmed label write.
    pub fn adopt_terminal(&mut self, prepared: PreparedAdoption) -> AdoptionEffect {
        let terminal = prepared.terminal;
        if self.endpoint() != &terminal.endpoint || self.client.socket().socket() != prepared.socket
        {
            return AdoptionEffect {
                terminal,
                result: Effect::NotApplied {
                    reason: "hc placement or socket does not match this operation".into(),
                },
            };
        }
        let params = prepared.params;
        let (frame, result) = self.client.exchange("terminal.label", &params, true);
        let result = match result {
            Ok(value) => {
                let labels = value.get("labels").and_then(Value::as_object);
                let expected = params["labels"].as_object().expect("prepared labels");
                let confirmed = value.get("address").and_then(Value::as_str)
                    == Some(terminal.key.as_str())
                    && value.get("state").and_then(Value::as_str) == Some("live")
                    && labels.is_some_and(|labels| {
                        expected
                            .iter()
                            .all(|(key, value)| labels.get(key) == Some(value))
                    });
                self.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: "terminal.label".into(),
                        params,
                        response: frame,
                    },
                    outcome: if confirmed {
                        CommandOutcome::Acknowledged
                    } else {
                        CommandOutcome::Uncertain
                    },
                });
                if confirmed {
                    Effect::Applied {
                        value: terminal.clone(),
                    }
                } else {
                    Effect::Uncertain {
                        reason: "hc placement was not confirmed".into(),
                    }
                }
            }
            Err(failure) => {
                let submitted = !failure.error.commands.is_empty();
                let rejected = matches!(
                    failure.rejection.as_deref(),
                    Some("invalid_params" | "invalid_request" | "method_not_found")
                );
                self.commands.extend(failure.error.commands);
                if !submitted {
                    Effect::NotApplied {
                        reason: "hc placement did not begin".into(),
                    }
                } else if rejected {
                    Effect::NotApplied {
                        reason: "hc placement was rejected".into(),
                    }
                } else {
                    Effect::Uncertain {
                        reason: "hc placement was not confirmed".into(),
                    }
                }
            }
        };
        AdoptionEffect { terminal, result }
    }

    /// Submit one prepared start once; an unconfirmed result is never replayed.
    pub fn start_terminal(&mut self, prepared: PreparedTerminal) -> Effect<NativeRef> {
        if self.endpoint() != &prepared.endpoint || self.client.socket().socket() != prepared.socket
        {
            return Effect::NotApplied {
                reason: "hc placement or socket does not match this operation".into(),
            };
        }
        let params = prepared.params;
        let (frame, result) = self.client.exchange("terminal.run", &params, true);
        match result {
            Ok(value) => {
                let address = value
                    .get("address")
                    .and_then(Value::as_str)
                    .filter(|address| {
                        !address.is_empty()
                            && !address.contains('/')
                            && crate::model::check_text(address, "terminal", false).is_ok()
                    });
                let labels = value.get("labels").and_then(Value::as_object);
                let expected = params["labels"].as_object().expect("prepared labels");
                let started = match value.get("state").and_then(Value::as_str) {
                    Some("live") => {
                        value
                            .get("pid")
                            .and_then(Value::as_u64)
                            .is_some_and(|pid| pid > 0 && u32::try_from(pid).is_ok())
                            && value.get("reason").is_none_or(Value::is_null)
                    }
                    Some("finished") => match value.get("reason").and_then(Value::as_str) {
                        Some("exited") => value
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .is_some_and(|code| i32::try_from(code).is_ok()),
                        Some("signaled") => value
                            .get("signal")
                            .and_then(Value::as_i64)
                            .is_some_and(|signal| signal > 0 && i32::try_from(signal).is_ok()),
                        _ => false,
                    },
                    _ => false,
                };
                let argv_matches = match params.get("argv") {
                    Some(argv) => value.get("argv") == Some(argv),
                    None => value
                        .get("argv")
                        .and_then(Value::as_array)
                        .is_some_and(|argv| {
                            argv.first()
                                .and_then(Value::as_str)
                                .is_some_and(|program| !program.is_empty())
                                && argv.iter().all(Value::is_string)
                        }),
                };
                let exact = value.get("cwd") == params.get("cwd")
                    && argv_matches
                    && value.get("parent").is_none_or(Value::is_null);
                let confirmed = started
                    && exact
                    && address.is_some()
                    && labels.is_some_and(|labels| {
                        expected
                            .iter()
                            .all(|(key, value)| labels.get(key) == Some(value))
                    });
                self.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: "terminal.run".into(),
                        params,
                        response: frame,
                    },
                    outcome: if confirmed {
                        CommandOutcome::Acknowledged
                    } else {
                        CommandOutcome::Uncertain
                    },
                });
                if confirmed {
                    Effect::Applied {
                        value: NativeRef {
                            adapter: FrontendId::Hc,
                            endpoint: self.endpoint().clone(),
                            key: address.expect("confirmed address").into(),
                        },
                    }
                } else {
                    Effect::Uncertain {
                        reason: "hc terminal start was not confirmed".into(),
                    }
                }
            }
            Err(failure) => {
                let submitted = !failure.error.commands.is_empty();
                let rejected = matches!(
                    failure.rejection.as_deref(),
                    Some("invalid_params" | "invalid_request" | "method_not_found")
                );
                self.commands.extend(failure.error.commands);
                if !submitted {
                    Effect::NotApplied {
                        reason: "hc terminal start did not begin".into(),
                    }
                } else if rejected {
                    Effect::NotApplied {
                        reason: "hc terminal start was rejected".into(),
                    }
                } else {
                    Effect::Uncertain {
                        reason: "hc terminal start was not confirmed".into(),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::EndpointId;

    #[test]
    fn empty_stream_shell_uses_fresh_placement_without_caller_identity() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        let caller = BTreeMap::from([
            ("HOME".into(), cwd.display().to_string()),
            ("PATH".into(), "/bin:/usr/bin".into()),
            (
                "HC_SOCKET".into(),
                cwd.join("core.sock").display().to_string(),
            ),
            ("HC_SESSION".into(), "selected".into()),
            ("HC_TERMINAL_ID".into(), "caller".into()),
            ("TOKEN".into(), "not forwarded".into()),
        ]);
        let socket = HcEndpoint::resolve(&caller, &cwd).unwrap();
        let scope = WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        };
        let space = NativeRef {
            adapter: FrontendId::Hc,
            endpoint: EndpointId("core".into()),
            key: "space".into(),
        };
        let item = LaunchItem::Shell { cwd: cwd.clone() };
        let prepare = || {
            PreparedTerminal::prepare(
                &item,
                StartContext {
                    scope: &scope,
                    stream: "work",
                    space: &space,
                    socket: &socket,
                    environment: &caller,
                },
            )
            .unwrap()
        };
        let first = prepare();
        let second = prepare();
        assert_ne!(first.tab(), second.tab());
        assert!(uuid::Uuid::parse_str(first.tab()).is_ok());
        assert_eq!(first.params["cwd"], json!(cwd));
        assert!(first.params.get("argv").is_none());
        assert_eq!(first.params["no_parent"], true);
        assert_eq!(first.params["labels"]["herdr.space"], "space");
        assert_eq!(first.params["labels"]["herdr.tab"], first.tab());
        assert_eq!(
            first.params["env"]["HC_SOCKET"],
            socket.socket().to_str().unwrap()
        );
        assert_eq!(first.params["env"]["HC_SESSION"], "selected");
        assert!(first.params["env"].get("HC_TERMINAL_ID").is_none());
        assert!(first.params["env"].get("TOKEN").is_none());
        let route: Value =
            serde_json::from_str(first.params["labels"]["streams.work"].as_str().unwrap()).unwrap();
        assert_eq!(route, json!({"version":1,"scope":scope,"stream":"work"}));
    }

    #[test]
    fn preparation_rejects_bad_execution_inputs_before_effects() {
        use crate::{
            integration::VerifiedLaunch, model::Session, runtime::conversations::ConversationRoots,
        };
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        let transcript = cwd.join("session.jsonl");
        std::fs::write(&transcript, "{\"type\":\"session\",\"id\":\"saved\"}\n").unwrap();
        let session = Session::new("session", "host", "work", "pi", "saved", "Saved").unwrap();
        let scope = WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        };
        let space = NativeRef {
            adapter: FrontendId::Hc,
            endpoint: EndpointId("core".into()),
            key: "space".into(),
        };
        let caller = BTreeMap::from([
            (
                "HC_SOCKET".into(),
                cwd.join("core.sock").display().to_string(),
            ),
            ("HC_SESSION".into(), "chosen".into()),
        ]);
        let socket = HcEndpoint::resolve(&caller, &cwd).unwrap();
        let prepare = |env: BTreeMap<String, String>| {
            let item = LaunchItem::Conversation(
                VerifiedLaunch::prepare(
                    &session,
                    &transcript,
                    &ConversationRoots::default(),
                    &cwd,
                    env,
                )
                .unwrap(),
            );
            PreparedTerminal::prepare(
                &item,
                StartContext {
                    scope: &scope,
                    stream: "work",
                    space: &space,
                    socket: &socket,
                    environment: &caller,
                },
            )
        };
        // /bin/sh is used as an executable stand-in; the prepared Session still supplies Pi argv.
        let bin = cwd.join("pi");
        std::os::unix::fs::symlink("/bin/sh", &bin).unwrap();
        let base = BTreeMap::from([
            ("PATH".into(), cwd.display().to_string()),
            ("STREAMS_STREAM_ID".into(), "work".into()),
        ]);
        assert!(prepare(base.clone()).is_ok());
        for (key, value) in [
            ("HC_TERMINAL_ID", "caller"),
            ("HC_PARENT_ID", "parent"),
            ("HC_SESSION", "other"),
            ("STREAMS_STREAM_ID", "other"),
            ("PATH", "/definitely-missing"),
        ] {
            let mut env = base.clone();
            env.insert(key.into(), value.into());
            assert!(prepare(env).is_err(), "must reject {key}");
        }
        let mut foreign = space.clone();
        foreign.key = "other/t1".into();
        let shell = LaunchItem::Shell { cwd: cwd.clone() };
        assert!(
            PreparedTerminal::prepare(
                &shell,
                StartContext {
                    scope: &scope,
                    stream: "work",
                    space: &foreign,
                    socket: &socket,
                    environment: &caller
                }
            )
            .is_err()
        );
    }
}
