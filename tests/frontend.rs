//! Native adapters are exercised only through scripted sockets, never live servers.

use crate::support::herdr::Scripted;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use streams::frontend::herdr::{HerdrClient, HerdrEndpoint, HerdrTopology};
use streams::frontend::*;

fn context(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}

fn herdr(fake: &Scripted, extra: &[(&str, &str)]) -> HerdrClient {
    let mut env = fake.environment();
    env.extend(context(extra));
    HerdrClient::connect(env, 0.5).unwrap()
}

fn result(kind: &str, field: &str, value: Value) -> Value {
    let mut result = json!({"type":kind});
    result[field] = value;
    result
}

fn native_workspace(id: &str, title: &str) -> Value {
    json!({"workspace_id":id,"label":title})
}

fn snapshot() -> Value {
    json!({"workspaces":[{"workspace_id":"w1","label":"One"}],
        "tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"Working","number":1}],
        "panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-1","foreground_cwd":"/foreground","cwd":"/shell"}],
        "layouts":[{"tab_id":"w1:t1","workspace_id":"w1","panes":[{"pane_id":"w1:p1"}]}],
        "agents":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-1","agent":"pi","agent_session":{"id":"native-session"}}]})
}

#[test]
fn selection_preserves_auto_none_explicit_and_override_policy() {
    let empty = BTreeMap::new();
    assert_eq!(
        resolve_frontend(FrontendMode::Auto, None, &empty)
            .unwrap()
            .resolved,
        None
    );
    let both = context(&[
        ("CMUX_WORKSPACE_ID", "c1"),
        ("HERDR_ENV", "1"),
        ("HERDR_PANE_ID", "w1:p1"),
    ]);
    assert!(resolve_frontend(FrontendMode::Auto, None, &both).is_err());
    assert!(resolve_frontend(FrontendMode::Auto, Some(FrontendRequest::Auto), &both).is_err());
    assert!(
        resolve_frontend(
            FrontendMode::Auto,
            None,
            &context(&[("CMUX_WORKSPACE_ID", "c1"), ("HC_TERMINAL_ID", "t1")])
        )
        .is_err()
    );
    assert!(
        resolve_frontend(
            FrontendMode::Auto,
            None,
            &context(&[
                ("HERDR_ENV", "1"),
                ("HERDR_PANE_ID", "w1:p1"),
                ("HC_TERMINAL_ID", "t1")
            ])
        )
        .is_err()
    );
    assert_eq!(
        resolve_frontend(
            FrontendMode::Auto,
            None,
            &context(&[("HC_SOCKET", "/tmp/hc.sock"), ("HC_SESSION", "default")])
        )
        .unwrap()
        .resolved,
        None
    );
    for values in [
        vec![("HERDR_ENV", "1")],
        vec![("HERDR_PANE_ID", "w1:p1")],
        vec![("HERDR_ENV", "0"), ("HERDR_PANE_ID", "w1:p1")],
    ] {
        assert!(resolve_frontend(FrontendMode::Auto, None, &context(&values)).is_err());
    }
    for id in [FrontendId::Cmux, FrontendId::Herdr, FrontendId::Hc] {
        assert_eq!(
            resolve_frontend(
                FrontendMode::Explicit(id),
                Some(FrontendRequest::Auto),
                &both
            )
            .unwrap()
            .resolved,
            Some(id)
        );
        assert_eq!(
            resolve_frontend(
                FrontendMode::None,
                Some(FrontendRequest::Explicit(id)),
                &both
            )
            .unwrap()
            .resolved,
            Some(id)
        );
    }
    assert_eq!(
        resolve_frontend(FrontendMode::None, None, &both)
            .unwrap()
            .resolved,
        None
    );
    for (values, id) in [
        (vec![("CMUX_WORKSPACE_ID", "c1")], FrontendId::Cmux),
        (
            vec![("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")],
            FrontendId::Herdr,
        ),
        (vec![("HC_TERMINAL_ID", "t1")], FrontendId::Hc),
    ] {
        assert_eq!(
            resolve_frontend(FrontendMode::Auto, None, &context(&values))
                .unwrap()
                .resolved,
            Some(id)
        );
    }
}

#[test]
fn herdr_endpoint_resolution_follows_herdr_and_fails_closed() {
    let resolve = |values: &[(&str, &str)]| HerdrEndpoint::resolve(&context(values));
    let endpoint = resolve(&[("HOME", "/home/test")]).unwrap();
    assert_eq!(
        endpoint.socket,
        Path::new("/home/test/.config/herdr/herdr.sock")
    );
    assert_eq!(endpoint.session, None);
    let endpoint = resolve(&[
        ("HOME", "/home/test"),
        ("XDG_CONFIG_HOME", "/xdg"),
        ("HERDR_SESSION", "work.1"),
    ])
    .unwrap();
    assert_eq!(
        endpoint.socket,
        Path::new("/xdg/herdr/sessions/work.1/herdr.sock")
    );
    assert_eq!(endpoint.session.as_deref(), Some("work.1"));
    let endpoint =
        resolve(&[("HERDR_SOCKET_PATH", "/run/h.sock"), ("HERDR_SESSION", "s")]).unwrap();
    assert_eq!(endpoint.socket, Path::new("/run/h.sock"));
    for values in [
        vec![],
        vec![("HERDR_SOCKET_PATH", "relative.sock")],
        vec![("HOME", "relative")],
        vec![("HOME", "/home/test"), ("HERDR_SESSION", "..")],
        vec![("HOME", "/home/test"), ("HERDR_SESSION", "a/b")],
    ] {
        assert!(resolve(&values).is_err(), "{values:?}");
    }
    // A missing server is a connection error, not a fallback.
    assert!(
        HerdrClient::connect(
            context(&[("HERDR_SOCKET_PATH", "/nonexistent/h.sock")]),
            0.5
        )
        .is_err()
    );
}

#[test]
fn herdr_reader_context_cannot_change_the_pinned_socket_or_namespace() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[("HERDR_SESSION", "named")]);
    let mut reader = fake.environment();
    reader.insert("HERDR_SESSION".into(), "named".into());
    reader.insert("HERDR_PANE_ID".into(), "owner".into());
    assert!(client.with_context(reader.clone()).is_ok());
    for (key, value) in [
        ("HERDR_SOCKET_PATH", "/different.sock"),
        ("HERDR_SESSION", "other"),
        ("HERDR_SESSION", ""),
    ] {
        let mut changed = reader.clone();
        changed.insert(key.into(), value.into());
        assert!(client.with_context(changed).is_err(), "{key}={value}");
    }
    reader.remove("HERDR_SESSION");
    assert!(client.with_context(reader).is_err());
    assert_eq!(fake.server.methods(), ["ping"]);
}

#[test]
fn herdr_snapshot_caller_requires_one_valid_matching_workspace() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")]);
    let good = snapshot();
    assert!(client.current_pane_from(&good).is_ok());
    for workspaces in [
        Value::Null,
        json!({}),
        json!([]),
        json!([native_workspace("other", "Other")]),
        json!([good["workspaces"][0], good["workspaces"][0]]),
        json!([{"workspace_id":"w1","label":null}]),
    ] {
        let mut changed = good.clone();
        changed["workspaces"] = workspaces;
        assert!(client.current_pane_from(&changed).is_err(), "{changed}");
    }
    assert_eq!(fake.server.methods(), ["ping"]);
}

#[test]
fn herdr_complete_topology_preserves_exact_identities_and_native_agent_payload() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[]);
    fake.result(result("session_snapshot", "snapshot", snapshot()));
    let topology = client.topology().unwrap();
    assert_eq!(topology.panes[0].terminal_id, "term-1");
    assert_eq!(topology.panes[0].pane.cwd, "/foreground");
    assert_eq!(
        topology.agents[0].native["agent_session"]["id"],
        "native-session"
    );
    assert_eq!(fake.server.methods(), ["ping", "session.snapshot"]);
    fake.exhausted();
}

#[test]
fn herdr_topology_rejects_every_missing_collection_and_broken_relationship() {
    let good = snapshot();
    for field in ["workspaces", "tabs", "panes", "layouts", "agents"] {
        let mut bad = good.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(HerdrTopology::parse(&bad).is_err(), "{field}");
        let mut bad = good.clone();
        bad[field] = json!({});
        assert!(HerdrTopology::parse(&bad).is_err(), "{field}");
    }
    let mut bads = Vec::new();
    for field in ["workspaces", "tabs", "panes", "layouts"] {
        let mut bad = good.clone();
        let first = bad[field][0].clone();
        bad[field].as_array_mut().unwrap().push(first);
        bads.push(bad);
    }
    for (collection, key, value) in [
        ("tabs", "workspace_id", json!("missing")),
        ("tabs", "label", json!(null)),
        ("panes", "tab_id", json!("missing")),
        ("panes", "workspace_id", json!("other")),
        ("panes", "terminal_id", json!("")),
        ("panes", "cwd", json!(4)),
        ("layouts", "workspace_id", json!("other")),
        ("layouts", "panes", json!([])),
        ("layouts", "panes", json!([{"pane_id":"missing"}])),
        (
            "layouts",
            "panes",
            json!([{"pane_id":"w1:p1"},{"pane_id":"w1:p1"}]),
        ),
        ("agents", "tab_id", json!("other")),
        ("agents", "workspace_id", json!("other")),
        ("agents", "terminal_id", json!("other")),
    ] {
        let mut bad = good.clone();
        bad[collection][0][key] = value;
        bads.push(bad);
    }
    let mut no_layout = good.clone();
    no_layout["layouts"] = json!([]);
    bads.push(no_layout);
    for bad in bads {
        assert!(HerdrTopology::parse(&bad).is_err(), "accepted {bad}");
    }
}
