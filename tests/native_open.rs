//! Native-open model and pre-effect durability; no native service is contacted.
use crate::support::model::invariant_counterexample;
use serde_json::json;
use streams_temper::runtime::{
    RuntimeError,
    open_protocol::{NATIVE_OPEN_IOA, protocol_table},
    state::{Association, OpenEffect, RuntimeState},
};
use temper_jit::table::EvalContext;
use temper_verify::cascade::VerificationCascade;
const SCOPE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn native_open_passes_temper_cascade() {
    assert!(
        streams_temper::sync::protocol::verify_total_context(
            protocol_table(),
            &EvalContext::default()
        )
        .is_err()
    );
    let report = VerificationCascade::from_ioa(NATIVE_OPEN_IOA)
        .with_sim_seeds(5)
        .with_prop_test_cases(100)
        .run();
    assert_eq!(report.levels.len(), 4);
    assert!(report.all_passed, "{report:#?}");
}
#[test]
fn missing_pending_effect_has_temper_counterexample() {
    let broken = NATIVE_OPEN_IOA.replacen(
        "effect = [\"pending = true\"]",
        "effect = [\"pending = false\"]",
        1,
    );
    assert_ne!(broken, NATIVE_OPEN_IOA);
    assert!(invariant_counterexample(&broken).is_some());
}
#[test]
fn marker_precedes_effect_and_checked_result_precedes_ready_binding() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = RuntimeState::new(temp.path().join("runtime"), "host").unwrap();
    let mut stream = state.stream_lock(Some(SCOPE), "stream").unwrap();
    let mut attempt = stream.begin_attempt(None, "/chosen", None).unwrap();
    let workspace = attempt
        .effect(
            OpenEffect::WorkspaceCreate,
            || {
                let persisted = state.pending_attempt(SCOPE, "stream").unwrap().unwrap();
                assert_eq!(persisted.effects(), &[json!({"kind":"workspace_create"})]);
                assert!(state.association(SCOPE, "workspace").unwrap().is_none());
                Ok("workspace".to_string())
            },
            |id| json!({"workspace_id":id}),
        )
        .unwrap();
    assert_eq!(
        state
            .pending_attempt(SCOPE, "stream")
            .unwrap()
            .unwrap()
            .effects()[0]["returned"],
        json!({"workspace_id":"workspace"})
    );
    attempt
        .checkpoint(
            Some(&workspace),
            attempt.record().effects().to_vec(),
            vec![],
        )
        .unwrap();
    attempt
        .write_association(&Association::new("stream", &workspace, None, true).unwrap())
        .unwrap();
    assert!(
        attempt
            .effect(
                OpenEffect::TabCreate,
                || panic!("ready binding must prevent another effect"),
                |_: &()| json!(null)
            )
            .is_err()
    );
    attempt.complete().unwrap();
    assert!(state.pending_attempt(SCOPE, "stream").unwrap().is_none());
    assert!(
        state
            .association(SCOPE, &workspace)
            .unwrap()
            .unwrap()
            .ready()
    );
}
#[test]
fn ambiguous_effect_stays_fenced_even_if_caller_ignores_its_error() {
    let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let state = RuntimeState::new(temp.path().join("runtime"), "host").unwrap();
    let mut stream = state.stream_lock(Some(SCOPE), "stream").unwrap();
    stream
        .write_association(&Association::new("stream", "workspace", None, true).unwrap())
        .unwrap();
    let mut attempt = stream
        .begin_attempt(None, "/chosen", Some("workspace"))
        .unwrap();
    assert!(
        !state
            .association(SCOPE, "workspace")
            .unwrap()
            .unwrap()
            .ready()
    );
    let result = attempt.effect(
        OpenEffect::TabCreate,
        || Err::<(), _>(RuntimeError::Invalid("lost native acknowledgement".into())),
        |_| json!(null),
    );
    assert!(result.is_err());
    assert_eq!(
        state
            .pending_attempt(SCOPE, "stream")
            .unwrap()
            .unwrap()
            .effects()[0]["error"]["message"],
        "invalid host-local state: lost native acknowledgement"
    );
    assert!(
        attempt
            .effect(
                OpenEffect::WorkspaceCreate,
                || panic!("must never retry ambiguous effect"),
                |_: &()| json!(null)
            )
            .is_err()
    );
    assert!(
        attempt
            .write_association(&Association::new("stream", "workspace", None, true).unwrap())
            .is_err()
    );
    drop(attempt);
    drop(stream);
    assert!(state.stream_lock(Some(SCOPE), "stream").is_err());
    assert!(state.stream_lock(None, "stream").is_err());
}
