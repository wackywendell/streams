use super::*;
use serde_json::json;

fn checkout(id: &str, url: Option<&str>) -> CheckoutLocation {
    CheckoutLocation {
        id: id.into(),
        directory: format!("/{id}").into(),
        repository_urls: url.into_iter().map(str::to_owned).collect(),
    }
}

#[test]
fn selected_precedence_explicit_defaults_and_ambiguity() {
    let mut state = State::new("host-identity");
    let selected = checkout("selected", Some("repo"));
    let fallback = checkout("fallback", Some("repo"));
    let external = checkout("external", Some("external"));
    state.checkouts = vec![selected.clone(), fallback.clone(), external.clone()];
    assert!(matches!(
        state.resolve("external", None, None),
        Err(RuntimeError::NotFound(_))
    ));
    match state.resolve("repo", None, None).unwrap_err() {
        RuntimeError::Ambiguous { candidates, .. } => {
            assert_eq!(candidates, [selected.id.clone(), fallback.id.clone()])
        }
        error => panic!("{error}"),
    }
    state.set_default("repo", &fallback.id).unwrap();
    state.set_default("external", &external.id).unwrap();
    assert_eq!(
        state
            .resolve("repo", Some(&selected.id), Some("repo"))
            .unwrap(),
        &selected
    );
    assert_eq!(
        state.resolve("repo", Some(&selected.id), None).unwrap(),
        &selected
    );
    assert_eq!(
        state
            .resolve("external", Some(&selected.id), Some("repo"))
            .unwrap(),
        &external
    );
    assert!(
        state
            .resolve("external", Some(&selected.id), Some("external"))
            .is_err()
    );
    assert!(state.resolve("repo", None, Some("repo")).is_err());
    let unbound = checkout("unbound", None);
    state.checkouts.push(unbound.clone());
    assert!(state.resolve("repo", Some(&unbound.id), None).is_err());
    let before = serde_json::to_value(&state).unwrap();
    assert!(state.set_default("external", &selected.id).is_err());
    assert_eq!(serde_json::to_value(&state).unwrap(), before);
}

#[test]
fn malformed_registry_schema_and_bindings_are_rejected_without_io() {
    let mut state = State::new("host-identity");
    state.checkouts.push(checkout("checkout", Some("repo")));
    let good = serde_json::to_value(&state).unwrap();
    let mut cases: Vec<Vec<u8>> = [
        "not JSON",
        "[]",
        "null",
        "{\"version\":1,\"version\":1}",
        "{\"bad\":NaN}",
    ]
    .iter()
    .map(|value| value.as_bytes().to_vec())
    .collect();
    for (key, value) in [
        ("version", json!(2)),
        ("version", json!(true)),
        ("version", json!(1.0)),
        ("host_id", json!("other-host")),
        ("checkouts", json!({})),
        ("defaults", json!([])),
        ("extra", json!(1)),
    ] {
        let mut state = good.clone();
        state[key] = value;
        cases.push(serde_json::to_vec(&state).unwrap());
    }
    for (key, value) in [
        ("id", json!("")),
        ("id", json!("bad\x7f")),
        ("id", json!(null)),
        ("directory", json!("relative")),
        ("directory", json!("//x/y")),
        ("directory", json!("/x/../y")),
        ("directory", json!("/x/./y")),
        ("directory", json!("/x//y")),
        ("directory", json!("/x/y/")),
        ("device", json!(true)),
        ("device", json!(10)),
        ("inode", json!(-1)),
        ("inode", json!(20)),
        ("repository_urls", json!(["repo", "repo"])),
        ("repository_urls", json!([""])),
        ("repository_urls", json!("repo")),
        ("extra", json!(1)),
    ] {
        let mut state = good.clone();
        state["checkouts"][0][key] = value;
        cases.push(serde_json::to_vec(&state).unwrap());
    }
    let mut state = good.clone();
    state["checkouts"]
        .as_array_mut()
        .unwrap()
        .push(good["checkouts"][0].clone());
    cases.push(serde_json::to_vec(&state).unwrap());
    let mut state = good.clone();
    state["defaults"] = json!({"repo": "not-registered"});
    cases.push(serde_json::to_vec(&state).unwrap());
    let mut state = good.clone();
    state["checkouts"][0] = json!(["id", "path", [], 1, 2]);
    cases.push(serde_json::to_vec(&state).unwrap());
    let encoded = serde_json::to_string(&good).unwrap();
    cases.push(
        encoded
            .replace(
                "\"defaults\":{}",
                "\"defaults\":{\"repo\":\"x\",\"repo\":\"y\"}",
            )
            .into_bytes(),
    );
    cases.push(
        encoded
            .replace("\"version\":1", "\"version\":1,\"version\":1")
            .into_bytes(),
    );
    cases.push(
        encoded
            .replace(
                "\"repository_urls\":[\"repo\"]",
                "\"repository_urls\":[\"repo\"],\"repository_urls\":[\"repo\"]",
            )
            .into_bytes(),
    );
    for bytes in cases {
        let decoded =
            crate::runtime::decode::<State>(&bytes).and_then(|state| state.check("host-identity"));
        assert!(
            decoded.is_err(),
            "accepted {}",
            String::from_utf8_lossy(&bytes)
        );
    }
}
