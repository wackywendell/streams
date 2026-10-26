use super::*;
use crate::runtime::state::{PlacementBinding, RuntimeState};
use serde_json::{Value, json};

const SCOPE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn placement() -> NativePlacement {
    NativePlacement::new(
        "stream",
        SCOPE,
        PlacementBinding::herdr("workspace", "lifetime").unwrap(),
    )
    .unwrap()
}
fn tokens() -> Tokens {
    BTreeMap::from([
        (
            "strm_catalog".into(),
            digest(&serde_json::to_vec("catalog").unwrap()),
        ),
        ("strm_host".into(), "host".into()),
        ("strm_stream".into(), "stream".into()),
        ("strm_binding".into(), "lifetime".into()),
    ])
}
fn snapshot(tokens: &Tokens) -> HerdrNativeSnapshot {
    HerdrNativeSnapshot::parse(&json!({
        "workspaces":[{"workspace_id":"workspace","label":"Work","number":1,"tokens":tokens}],
        "tabs":[],"panes":[],"agents":[],"layouts":[]
    }))
    .unwrap()
}

#[test]
fn each_agreeing_partial_projection_stays_unready_and_selects_only_missing_fields() {
    let placement = placement();
    let desired = tokens();
    for mask in 0u32..16 {
        let paint = desired
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, (k, v))| (k.clone(), v.clone()))
            .collect();
        let snapshot = snapshot(&paint);
        let observed =
            HerdrPlacementObservation::new("catalog", "host", SCOPE, &placement, &snapshot)
                .unwrap();
        assert_eq!(
            observed.missing_tokens().len(),
            4 - mask.count_ones() as usize
        );
        for (key, value) in observed.missing_tokens() {
            assert_eq!(desired.get(key), Some(value));
            assert!(!paint.contains_key(key));
        }
        let ready = observed.require_ready();
        assert_eq!(ready.is_ok(), mask == 15);
        if let Ok(ready) = ready {
            assert!(std::ptr::eq(ready.snapshot(), &snapshot));
            assert!(std::ptr::eq(ready.placement(), &placement));
            assert_eq!(ready.host_id(), "host");
            assert_eq!(ready.workspace_id(), "workspace");
        }
    }
}

#[test]
fn present_conflicts_are_not_missing_projection_or_adoption_permission() {
    let placement = placement();
    for key in tokens().keys() {
        for wrong in ["different", ""] {
            let mut paint = tokens();
            paint.insert(key.clone(), wrong.into());
            let snapshot = snapshot(&paint);
            assert!(
                HerdrPlacementObservation::new("catalog", "host", SCOPE, &placement, &snapshot)
                    .is_err()
            );
        }
    }
    let mut paint = tokens();
    paint.insert("strm_goal_short".into(), "Observed summary".into());
    paint.insert("foreign".into(), "unrelated metadata".into());
    let snapshot = snapshot(&paint);
    assert!(
        HerdrPlacementObservation::new("catalog", "host", SCOPE, &placement, &snapshot)
            .unwrap()
            .require_ready()
            .is_ok()
    );
}

#[test]
fn foreign_host_endpoint_missing_workspace_and_cmux_capability_refuse() {
    let placement = placement();
    let snapshot = snapshot(&tokens());
    for (source, host, scope) in [
        ("catalog", "foreign", SCOPE),
        ("catalog", "host", OTHER),
        ("different", "host", SCOPE),
    ] {
        assert!(
            HerdrPlacementObservation::new(source, host, scope, &placement, &snapshot).is_err()
        );
    }
    let missing = HerdrNativeSnapshot::parse(
        &json!({"workspaces":[],"tabs":[],"panes":[],"agents":[],"layouts":[]}),
    )
    .unwrap();
    assert!(
        HerdrPlacementObservation::new("catalog", "host", SCOPE, &placement, &missing).is_err()
    );
    let cmux = NativePlacement::new(
        "stream",
        SCOPE,
        PlacementBinding::cmux("workspace", None).unwrap(),
    )
    .unwrap();
    assert!(HerdrPlacementObservation::new("catalog", "host", SCOPE, &cmux, &snapshot).is_err());
}

#[test]
fn retained_pending_refuses_even_with_matching_paint_or_unknown_workspace_and_does_no_io() {
    let temp = tempfile::tempdir().unwrap();
    let state = RuntimeState::new(temp.path().join("runtime"), "host").unwrap();
    let path = state.placement_path("stream").unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let snapshot = snapshot(&tokens());
    for known in [false, true] {
        let value = json!({"version":2,"stream_id":"stream","scope":SCOPE,
            "binding":if known { json!({"frontend":"herdr","workspace_id":"workspace","nonce":"lifetime"}) } else { Value::Null },
            "pending":{"session_id":null,"command":null,"cwd":"/chosen","effects":[],"launches":[]}});
        std::fs::write(&path, value.to_string()).unwrap();
        let placement = state.placement(SCOPE, "stream").unwrap().unwrap();
        let saved = temp.path().join(format!("saved-{known}"));
        std::fs::rename(state.root(), &saved).unwrap();
        assert!(
            HerdrPlacementObservation::new("catalog", "host", SCOPE, &placement, &snapshot)
                .is_err()
        );
        assert!(!state.root().exists());
        std::fs::rename(saved, state.root()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), value.to_string());
    }
}
