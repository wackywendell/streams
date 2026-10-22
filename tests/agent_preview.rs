use serde_json::{Value, json};
use streams_temper::{
    agent::{EnrollmentRequest, effort_id_for, preview, session_id_for},
    codec::record_to_data,
    model::{Record, Session, Stream},
    operations::Actor,
    runtime::conversations::Harness,
};

fn context() -> Value {
    let mut stream = Stream::new("effort", "Effort", Some("Detailed goal".into()), "host").unwrap();
    stream.goal_short = Some("Goal".into());
    let mut session =
        Session::new("session", "host", "effort", "pi", "native", "Contribution").unwrap();
    session.state = "Detailed state".into();
    session.state_short = Some("State".into());
    let other = Session::new("other", "host", "effort", "pi", "other-native", "Other").unwrap();
    let stream = record_to_data(&Record::Stream(stream));
    let session = record_to_data(&Record::Session(session));
    let other = record_to_data(&Record::Session(other));
    json!({
        "catalog_source": "physical-source", "host_id": "host", "stream": stream,
        "session": session, "sessions": [other, session],
        "authority": {"host_id": "host", "token": "captured-token"},
        "checkouts": [], "native": {"opaque": "preserved, not attested"},
    })
}

fn empty() -> Value {
    let mut value = context();
    value["stream"] = Value::Null;
    value["session"] = Value::Null;
    value["sessions"] = json!([]);
    value["authority"] = Value::Null;
    value
}

fn enrollment() -> Value {
    json!({
        "operation_id": "enroll", "catalog_source": "physical-source", "harness": "pi",
        "harness_session_id": "native", "session": {"title": "Contribution"},
        "actor": "automatic", "cwd": "/does/not/exist/and/is/not/read",
    })
}

fn update(context: &Value, kind: &str, changes: Value, actor: Actor) -> Value {
    json!({
        "operation_id": "update", "catalog_source": context["catalog_source"],
        "kind": kind, "actor": actor, "before": context[kind], "changes": changes,
        "ownership_token": if kind == "session" || actor == Actor::Automatic {
            context["authority"]["token"].clone()
        } else { Value::Null },
        "inputs": if kind == "session" {
            json!([{"kind": "stream", "record": context["stream"]}])
        } else { json!([]) },
    })
}

fn envelope(context: &Value, action: &str, request: &Value) -> Value {
    json!({"catalog": context, "action": action, "request": request})
}

fn project(context: &Value, action: &str, request: &Value) -> Value {
    preview(&envelope(context, action, request).to_string()).unwrap()
}

fn reject(context: &Value, action: &str, request: &Value) {
    let source = envelope(context, action, request).to_string();
    assert!(preview(&source).is_err(), "unexpectedly accepted {source}");
}

fn simulated(result: &Value) {
    assert_eq!(result["simulated"], true);
    assert_eq!(result["local"], "unchanged");
    assert_eq!(result["sharing"], "local-only");
}

#[test]
fn enrollment_parser_and_constructors_share_typed_identity_and_naming_rules() {
    let mut wire = enrollment();
    wire["actor"] = json!("user");
    wire["stream"] = json!({"title": "New", "goal": "Goal", "goal_short": "Compact"});
    let parsed = EnrollmentRequest::parse(&wire.to_string(), "physical-source").unwrap();
    assert_eq!(parsed.operation_id(), "enroll");
    assert_eq!(parsed.catalog_source(), "physical-source");
    assert_eq!(parsed.harness(), Harness::Pi);
    assert_eq!(parsed.harness_session_id(), "native");
    assert_eq!(parsed.stream_id(), None);
    assert_eq!(parsed.session_title(), "Contribution");
    assert_eq!(parsed.cwd(), "/does/not/exist/and/is/not/read");
    assert_eq!(parsed.actor(), Actor::User);
    let proposal = parsed.stream_proposal().unwrap();
    assert_eq!(proposal.title(), "New");
    assert_eq!(proposal.goal(), Some("Goal"));
    assert_eq!(proposal.goal_short(), Some("Compact"));
    let stream = record_to_data(&Record::Stream(parsed.stream("host", "chosen-id").unwrap()));
    assert_eq!(stream["title_mode"], "manual");
    assert_eq!(stream["goal_mode"], "manual");
    assert_eq!(stream["goal_short"], "Compact");
    let session = parsed.session("host", "chosen-id").unwrap();
    assert_eq!(session.id, session_id_for("host", Harness::Pi, "native"));
    assert_eq!(session.stream_id, "chosen-id");
    assert_eq!(session.title_mode.to_string(), "manual");
    assert!(session.checkout.is_none());
    assert!(parsed.stream("", "chosen-id").is_err());
    assert!(parsed.session("host", "").is_err());

    wire["stream"] = json!({"title": "No goal"});
    let parsed = EnrollmentRequest::parse(&wire.to_string(), "physical-source").unwrap();
    assert_eq!(
        parsed.stream("host", "id").unwrap().goal_mode.to_string(),
        "automatic"
    );
    wire["stream"] = json!({"title": "Invalid", "goal_short": "Orphaned summary"});
    assert!(EnrollmentRequest::parse(&wire.to_string(), "physical-source").is_err());
}

#[test]
fn enrollment_rejects_native_actor_even_when_no_records_would_be_created() {
    let mut request = enrollment();
    request["actor"] = json!("native");
    request["stream_id"] = json!("effort");
    assert!(EnrollmentRequest::parse(&request.to_string(), "physical-source").is_err());
    reject(&context(), "enroll", &request);
    request["stream"] = json!({"title": "New"});
    reject(&empty(), "enroll", &request);
}

#[test]
fn effort_and_preview_ids_use_python_ascii_json_and_separate_namespaces() {
    let host = "høst";
    let native = "nâtive/😀\u{7f}";
    let live_name = r#"streams:effort:["h\u00f8st", "pi", "n\u00e2tive/\ud83d\ude00\u007f"]"#;
    let expected = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, live_name.as_bytes()).to_string();
    assert_eq!(effort_id_for(host, Harness::Pi, native), expected);
    assert_ne!(effort_id_for(host, Harness::Codex, native), expected);
    let mut capture = empty();
    capture["host_id"] = json!(host);
    let mut request = enrollment();
    request["operation_id"] = json!("op/😀");
    request["harness_session_id"] = json!(native);
    request["stream"] = json!({"title": "New"});
    let result = project(&capture, "enroll", &request);
    let preview_name = r#"streams:preview:effort:["op/\ud83d\ude00", "h\u00f8st", "pi", "n\u00e2tive/\ud83d\ude00\u007f"]"#;
    let expected_preview =
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, preview_name.as_bytes()).to_string();
    assert_eq!(result["catalog"]["stream"]["id"], expected_preview);
    assert_ne!(result["catalog"]["stream"]["id"], expected);
}

#[test]
fn deterministic_enrollment_supports_incremental_session_and_effort_replay() {
    let capture = empty();
    let original = capture.clone();
    let mut request = enrollment();
    request["stream"] = json!({"title": "New", "goal": "Goal", "goal_short": "Compact"});
    let first = project(&capture, "enroll", &request);
    assert_eq!(first, project(&capture, "enroll", &request));
    assert_eq!(capture, original);
    simulated(&first);
    simulated(&first["receipt"]);
    assert_eq!(first["receipt"]["saves"].as_array().unwrap().len(), 2);
    for saved in first["receipt"]["saves"].as_array().unwrap() {
        simulated(&saved["save"]);
        assert_eq!(saved["save"]["domain"]["outcome"], "changes");
    }
    assert!(
        first["catalog"]["authority"]["token"]
            .as_str()
            .unwrap()
            .contains("redacted-preview")
    );
    let context = &first["catalog"];
    let session_update = update(
        context,
        "session",
        json!({"state": "Follow-on", "state_short": "Ready"}),
        Actor::Automatic,
    );
    let second = project(context, "apply", &session_update);
    assert_eq!(second["catalog"]["session"]["state_short"], "Ready");
    simulated(&second["receipt"]["save"]);
    let mut effort_update = update(
        &second["catalog"],
        "stream",
        json!({"goal": "Refined goal"}),
        Actor::Automatic,
    );
    effort_update["inputs"] = json!([{"kind": "session", "record": second["catalog"]["session"]}]);
    let third = project(&second["catalog"], "apply", &effort_update);
    assert_eq!(third["catalog"]["stream"]["goal"], "Refined goal");
    assert_eq!(third["catalog"]["stream"]["goal_short"], Value::Null);
    assert_eq!(third["catalog"]["session"], second["catalog"]["session"]);
    assert_eq!(third["catalog"]["native"], capture["native"]);
}

#[test]
fn apply_preserves_supplied_order_and_does_not_modify_its_source() {
    let capture = context();
    let request = update(
        &capture,
        "session",
        json!({"remaining": "Next"}),
        Actor::Automatic,
    );
    let source = envelope(&capture, "apply", &request).to_string();
    let original = source.clone();
    let result = preview(&source).unwrap();
    assert_eq!(source, original);
    assert_eq!(capture, context());
    assert_eq!(result["catalog"]["session"]["state_short"], Value::Null);
    assert_eq!(result["catalog"]["sessions"][0], capture["sessions"][0]);
    assert_eq!(
        result["catalog"]["sessions"][1],
        result["catalog"]["session"]
    );
    assert_eq!(result["catalog"]["checkouts"], capture["checkouts"]);
    assert_eq!(result["catalog"]["native"], capture["native"]);
}

#[test]
fn moved_primary_and_sibling_sessions_preview_without_loading_past_streams() {
    let mut capture = context();
    capture["session"]["previous_stream_ids"] = json!(["old-primary"]);
    capture["sessions"][1] = capture["session"].clone();
    capture["sessions"][0]["previous_stream_ids"] = json!(["old-sibling"]);
    let request = update(
        &capture,
        "session",
        json!({"remaining": "Next"}),
        Actor::Automatic,
    );
    let projected = project(&capture, "apply", &request);
    assert_eq!(projected["catalog"]["session"]["remaining"], "Next");
    assert_eq!(
        projected["catalog"]["session"]["previous_stream_ids"],
        json!(["old-primary"])
    );
    assert_eq!(projected["catalog"]["sessions"][0], capture["sessions"][0]);
    let request = update(
        &projected["catalog"],
        "stream",
        json!({"goal": "Refined"}),
        Actor::Automatic,
    );
    let projected = project(&projected["catalog"], "apply", &request);
    assert_eq!(projected["catalog"]["stream"]["goal"], "Refined");
    assert_eq!(projected["catalog"]["sessions"][0], capture["sessions"][0]);
}

#[test]
fn manual_protection_and_derivative_clearing_are_domain_owned() {
    let mut capture = context();
    capture["stream"]["title_mode"] = json!("manual");
    let request = update(
        &capture,
        "stream",
        json!({"title": "Ignored"}),
        Actor::Automatic,
    );
    let result = project(&capture, "apply", &request);
    assert_eq!(result["catalog"]["stream"]["title"], "Effort");
    assert_eq!(result["receipt"]["save"]["domain"]["outcome"], "no-change");
    let request = update(&capture, "stream", json!({"goal": null}), Actor::User);
    let result = project(&capture, "apply", &request);
    assert_eq!(result["catalog"]["stream"]["goal_short"], Value::Null);
    assert_eq!(result["catalog"]["stream"]["goal_mode"], "manual");
    let request = update(
        &capture,
        "session",
        json!({"status": "done", "state_short": "Finished"}),
        Actor::Automatic,
    );
    let result = project(&capture, "apply", &request);
    assert_eq!(result["catalog"]["session"]["state_short"], "Finished");
    let request = update(&capture, "session", json!({"title": "Chosen"}), Actor::User);
    let result = project(&capture, "apply", &request);
    assert_eq!(result["catalog"]["session"]["title_mode"], "manual");
}

#[test]
fn exact_before_inputs_source_and_authority_are_required() {
    let capture = context();
    let base = update(
        &capture,
        "session",
        json!({"title": "User intent"}),
        Actor::User,
    );
    let mut invalid = Vec::new();
    let mut request = base.clone();
    request["catalog_source"] = json!("elsewhere");
    invalid.push(request);
    let mut request = base.clone();
    request["before"]["state"] = json!("Stale independent field");
    invalid.push(request);
    let mut request = base.clone();
    request["before"]["id"] = json!("other");
    invalid.push(request);
    let mut request = base.clone();
    request["inputs"][0]["record"]["goal"] = json!("Stale goal");
    invalid.push(request);
    for token in [json!("other-period"), Value::Null] {
        let mut request = base.clone();
        request["ownership_token"] = token;
        invalid.push(request);
    }
    let mut request = base.clone();
    request["inputs"] = json!([base["inputs"][0], base["inputs"][0]]);
    invalid.push(request);
    for request in invalid {
        reject(&capture, "apply", &request);
    }
    assert_eq!(capture, context());
}

#[test]
fn foreign_producer_and_foreign_effective_authority_fail_closed() {
    let mut capture = context();
    capture["stream"]["owner"]["host_id"] = json!("foreign");
    let request = update(
        &capture,
        "stream",
        json!({"goal": "Inference"}),
        Actor::Automatic,
    );
    reject(&capture, "apply", &request);
    let mut capture = context();
    capture["authority"]["host_id"] = json!("foreign");
    let request = update(
        &capture,
        "session",
        json!({"state": "Next"}),
        Actor::Automatic,
    );
    reject(&capture, "apply", &request);
    capture["session"] = Value::Null;
    let mut request = enrollment();
    request["harness_session_id"] = json!("third-native");
    reject(&capture, "enroll", &request);
}

#[test]
fn retry_keeps_existing_identity_and_no_save_receipt_matches_stream_cli_grammar() {
    let mut capture = context();
    capture["session"] = Value::Null;
    let mut request = enrollment();
    request["harness_session_id"] = json!("third-native");
    request["stream_id"] = json!("effort");
    let first = project(&capture, "enroll", &request);
    assert_eq!(
        &first["catalog"]["sessions"].as_array().unwrap()[..2],
        capture["sessions"].as_array().unwrap()
    );
    assert_eq!(first["catalog"]["sessions"].as_array().unwrap().len(), 3);
    request["operation_id"] = json!("retry-is-not-a-new-identity");
    let repeated = project(&first["catalog"], "enroll", &request);
    assert_eq!(repeated["catalog"], first["catalog"]);
    let receipt = &repeated["receipt"];
    assert_eq!(receipt["saves"], json!([]));
    assert!(receipt.get("operation_id").is_none());
    assert!(receipt.get("save").is_none());
    assert!(receipt["stream"].is_object());
    assert!(receipt["session"].is_object());
    assert!(receipt["authority"].is_object());
    simulated(&receipt["publication"]);
    assert_eq!(receipt["publication"]["domain"], Value::Null);
    request["harness_session_id"] = json!("different-native");
    reject(&first["catalog"], "enroll", &request);
}

#[test]
fn new_automatic_enrollment_retry_never_mints_another_effort() {
    let mut request = enrollment();
    request["stream"] = json!({"title": "New"});
    let first = project(&empty(), "enroll", &request);
    request["operation_id"] = json!("retry");
    let repeated = project(&first["catalog"], "enroll", &request);
    assert_eq!(repeated["catalog"], first["catalog"]);
    assert_eq!(repeated["receipt"]["saves"], json!([]));
    // Preview authority is not live own-creation evidence for a user proposal replay.
    request["actor"] = json!("user");
    reject(&first["catalog"], "enroll", &request);
}

#[test]
fn preview_does_not_invent_creation_retry_evidence_from_a_token() {
    let mut request = enrollment();
    request["actor"] = json!("user");
    request["stream"] = json!({"title": "Existing"});
    let parsed = EnrollmentRequest::parse(&request.to_string(), "physical-source").unwrap();
    let mut capture = empty();
    capture["stream"] = record_to_data(&Record::Stream(parsed.stream("host", "effort").unwrap()));
    capture["authority"] = json!({"host_id": "host", "token": "enroll:stream"});
    let error = preview(&envelope(&capture, "enroll", &request).to_string()).unwrap_err();
    assert!(error.to_string().contains("effort already exists"));
}

#[test]
fn enrollment_never_moves_or_impersonates_supplied_membership() {
    let capture = context();
    for (field, value) in [
        ("stream_id", json!("other")),
        ("harness_session_id", json!("other-native")),
        ("harness", json!("codex")),
    ] {
        let mut request = enrollment();
        request[field] = value;
        reject(&capture, "enroll", &request);
    }
    let mut omitted_primary = capture.clone();
    omitted_primary["session"] = Value::Null;
    reject(&omitted_primary, "enroll", &enrollment());
    reject(&empty(), "enroll", &enrollment());
    let mut request = enrollment();
    request["stream_id"] = json!("unknown");
    request["stream"] = json!({"title": "New"});
    reject(&empty(), "enroll", &request);
}

#[test]
fn context_fields_and_identity_relationships_are_strict() {
    let capture = context();
    let request = update(&capture, "session", json!({}), Actor::Automatic);
    for field in [
        "catalog_source",
        "host_id",
        "stream",
        "session",
        "sessions",
        "authority",
    ] {
        let mut broken = capture.clone();
        broken.as_object_mut().unwrap().remove(field);
        reject(&broken, "apply", &request);
    }
    let mut invalid = Vec::new();
    let mut broken = capture.clone();
    broken["session"]["host_id"] = json!("foreign");
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["sessions"] = json!([capture["sessions"][0]]);
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["sessions"][0]["stream_id"] = json!("elsewhere");
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["sessions"] = json!([capture["session"], capture["session"]]);
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["sessions"][0]["harness_session_id"] = json!("native");
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["session"]["state"] = json!("Not the exact supplied member");
    invalid.push(broken);
    let mut broken = empty();
    broken["authority"] = capture["authority"].clone();
    invalid.push(broken);
    for broken in invalid {
        reject(&broken, "apply", &request);
    }
}

#[test]
fn unknown_fields_positional_arrays_and_map_shaped_enums_are_rejected() {
    let capture = context();
    let request = update(&capture, "session", json!({}), Actor::Automatic);
    let base = envelope(&capture, "apply", &request);
    let paths = [
        "/unexpected",
        "/catalog/unexpected",
        "/catalog/session/unexpected",
        "/catalog/stream/unexpected",
        "/catalog/authority/unexpected",
        "/request/unexpected",
    ];
    for path in paths {
        let mut wire = base.clone();
        let (parent, key) = path.rsplit_once('/').unwrap();
        wire.pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), json!(true));
        assert!(preview(&wire.to_string()).is_err(), "{path}");
    }
    for path in [
        "/catalog",
        "/catalog/session",
        "/catalog/stream",
        "/catalog/authority",
        "/catalog/sessions/0",
        "/request",
        "/request/before",
        "/request/inputs/0",
    ] {
        let mut wire = base.clone();
        *wire.pointer_mut(path).unwrap() = json!([]);
        assert!(preview(&wire.to_string()).is_err(), "{path}");
    }
    for (path, value) in [
        ("/action", json!({"apply": null})),
        ("/request/actor", json!({"automatic": null})),
        ("/request/kind", json!({"session": null})),
        ("/catalog/stream/status", json!({"active": null})),
        ("/catalog/stream/title_mode", json!({"automatic": null})),
    ] {
        let mut wire = base.clone();
        *wire.pointer_mut(path).unwrap() = value;
        assert!(preview(&wire.to_string()).is_err(), "{path}");
    }
    for source in ["null", "[]", "{}", r#"{"catalog":{},"catalog":{}}"#] {
        assert!(preview(source).is_err());
    }
    let source = base.to_string();
    let duplicated = source.replacen(
        "\"state\":\"Detailed state\"",
        "\"state\":\"Detailed state\",\"state\":\"duplicate\"",
        1,
    );
    assert_ne!(source, duplicated);
    assert!(preview(&duplicated).is_err());
}

#[test]
fn enrollment_request_shape_and_text_validation_match_the_live_boundary() {
    let base = enrollment();
    for field in [
        "operation_id",
        "catalog_source",
        "harness",
        "harness_session_id",
        "session",
        "actor",
        "cwd",
    ] {
        let mut request = base.clone();
        request.as_object_mut().unwrap().remove(field);
        assert!(
            EnrollmentRequest::parse(&request.to_string(), "physical-source").is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("unexpected", json!(true)),
        ("session", json!({"title": "X", "state": "No"})),
        ("session", json!(["Title"])),
        ("stream", json!(["Title", null, null])),
        ("stream", json!({"title": "X", "status": "done"})),
        ("cwd", Value::Null),
        ("cwd", json!("\u{0}")),
        ("harness", json!("unknown")),
        ("harness", json!({"pi": null})),
        ("actor", json!({"automatic": null})),
        ("actor", json!("unknown")),
        ("stream_id", json!(" ")),
        ("session", json!({"title": ""})),
        ("stream", json!({"title": ""})),
        (
            "stream",
            json!({"title": "X", "goal": null, "goal_short": "Orphan"}),
        ),
        ("catalog_source", json!("wrong")),
    ] {
        let mut request = base.clone();
        request[field] = value;
        assert!(
            EnrollmentRequest::parse(&request.to_string(), "physical-source").is_err(),
            "{request}"
        );
        reject(&context(), "enroll", &request);
    }
    let mut explicit_null = base.clone();
    explicit_null["stream"] = Value::Null;
    explicit_null["stream_id"] = Value::Null;
    EnrollmentRequest::parse(&explicit_null.to_string(), "physical-source").unwrap();
    let duplicate = base.to_string().replacen(
        "\"title\":\"Contribution\"",
        "\"title\":\"Contribution\",\"title\":\"duplicate\"",
        1,
    );
    assert!(EnrollmentRequest::parse(&duplicate, "physical-source").is_err());
}
