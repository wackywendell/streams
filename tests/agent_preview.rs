use serde_json::{Value, json};
use streams::{
    agent::{EnrollmentRequest, effort_id_for, preview, session_id_for},
    codec::record_to_data,
    model::{Record, Session, Stream, TitleDefaults},
    operations::Actor,
    runtime::conversations::Harness,
};

fn context() -> Value {
    let mut stream = Stream::new("effort", "Effort", Some("Detailed goal".into()), "host").unwrap();
    stream.goal_short = Some("Goal".into());
    let mut session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "effort",
        "pi",
        "native",
        "Contribution",
    )
    .unwrap();
    session.state = "Detailed state".into();
    session.state_short = Some("State".into());
    let stream = record_to_data(&Record::Stream(stream));
    let session = record_to_data(&Record::Session(session));
    json!({"catalog_source":"physical-source","host_id":"host","title_defaults":TitleDefaults::default(),
        "stream":stream,"session":session,"checkouts":[],
        "document_locations":[]})
}
fn empty() -> Value {
    let mut value = context();
    value["stream"] = Value::Null;
    value["session"] = Value::Null;
    value
}
fn enrollment() -> Value {
    json!({"version":2,"catalog_source":"physical-source","harness":"pi","harness_session_id":"native",
        "session":{"title":"Contribution"},"actor":"automatic","cwd":"/does/not/exist/and/is/not/read"})
}
fn update(context: &Value, kind: &str, changes: Value, actor: Actor) -> Value {
    json!({"version":2,"catalog_source":context["catalog_source"],"kind":kind,"actor":actor,
        "before":context[kind],"changes":changes,"title_default":context["title_defaults"][kind],
        "inputs":if kind=="stream" && actor==Actor::Automatic {
            json!([{"kind":"session","record":context["session"]}])
        } else {json!([])}})
}
fn envelope(context: &Value, action: &str, request: &Value) -> Value {
    json!({"catalog":context,"action":action,"request":request})
}
fn project(context: &Value, action: &str, request: &Value) -> Value {
    preview(&envelope(context, action, request).to_string()).unwrap()
}
/// The projected records, without the fields the projection derives from them.
fn records(catalog: &Value) -> Value {
    let mut records = catalog.clone();
    let map = records.as_object_mut().unwrap();
    map.remove("patch_schemas");
    map.remove("stream_status");
    records
}
fn reject(context: &Value, action: &str, request: &Value) {
    let source = envelope(context, action, request).to_string();
    assert!(preview(&source).is_err(), "unexpectedly accepted {source}");
}
fn simulated(result: &Value) {
    assert_eq!(result["simulated"], true);
    assert_eq!(result["local"], "unchanged");
    assert!(result.get("sharing").is_none());
    assert!(result.get("authority").is_none());
    assert!(result.get("publication").is_none());
}

#[test]
fn reference_preview_uses_the_same_membership_guard_after_its_own_session_report() {
    let mut captured = context();
    captured["stream"]["repositories"] = json!({"repo":{"url":"https://example.test/repo"}});
    let request = update(
        &captured,
        "stream",
        json!({"reference_changes":[{"key":{"kind":"document","repository_url":"https://example.test/repo","path":"docs/design.md"},"desired":{"name":"Design","purpose":"Guide work","role":"brief","state":"active","target":{"kind":"document","repository_url":"https://example.test/repo","ref":"main","path":"docs/design.md"}}}]}),
        Actor::Automatic,
    );
    let progress = update(
        &captured,
        "session",
        json!({"state":"Working"}),
        Actor::Automatic,
    );
    let after_progress = project(&captured, "apply", &progress)["catalog"].clone();
    let after = project(&after_progress, "apply", &request);
    assert_eq!(after["catalog"]["session"]["state"], "Working");
    let document = &after["catalog"]["stream"]["references"][0]["target"];
    assert_eq!(document["path"], "docs/design.md");
    assert_eq!(document["source_session_id"], captured["session"]["id"]);
    assert!(document.get("commit").is_none());
    assert_eq!(captured["stream"]["references"], json!([]));
    assert_eq!(after["receipt"]["save"]["domain"]["outcome"], "changes");
}

fn preview_reference(url: &str) -> Value {
    json!({"name":"Reference","purpose":"Guide work","role":"brief","state":"active",
        "target":{"kind":"link","url":url}})
}

#[test]
fn reference_preview_partial_correction_and_whole_refusal_keep_domain_rows() {
    let mut captured = context();
    captured["stream"]["references"] = json!([
        preview_reference("https://example.test/a"),
        preview_reference("https://example.test/b"),
        preview_reference("https://example.test/c")
    ]);
    let desired = preview_reference("https://example.test/b");
    let request = update(
        &captured,
        "stream",
        json!({"reference_changes":[
            {"key":{"kind":"url","url":"https://example.test/a"},"desired":desired},
            {"key":{"kind":"url","url":"https://example.test/c"},"desired":null}
        ]}),
        Actor::User,
    );
    let result = project(&captured, "apply", &request);
    let domain = &result["receipt"]["save"]["domain"];
    assert_eq!(domain["outcome"], "changes");
    assert_eq!(domain["reference_results"][0]["outcome"], "conflict");
    assert_eq!(domain["reference_results"][1]["outcome"], "changes");
    assert_eq!(
        result["catalog"]["stream"]["references"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let mut whole = request.clone();
    whole["changes"]["reference_changes"]
        .as_array_mut()
        .unwrap()
        .pop();
    let result = project(&captured, "apply", &whole);
    assert_eq!(result["receipt"]["save"]["domain"]["outcome"], "conflict");
    assert_eq!(
        result["receipt"]["save"]["domain"]["reference_results"][0]["reason"],
        "replacement target already exists"
    );
    assert_eq!(records(&result["catalog"]), captured);
    let mut changed_context = captured.clone();
    changed_context["stream"]["title"] = json!("Changed capture");
    reject(&changed_context, "apply", &request);
    // User references do not need a Session in either the request or the snapshot.
    captured["session"] = Value::Null;
    assert_eq!(
        project(&captured, "apply", &request)["receipt"]["save"]["domain"]["outcome"],
        "changes"
    );
}

#[test]
fn reference_preview_session_revisit_and_automatic_restoration_are_receipted() {
    let mut captured = context();
    let mut saved = preview_reference("https://example.test/dropped");
    saved["state"] = json!("dropped");
    captured["stream"]["references"] = json!([saved]);
    let desired = preview_reference("https://example.test/dropped");
    let request = update(
        &captured,
        "stream",
        json!({"reference_changes":[
            {"key":{"kind":"url","url":"https://example.test/dropped"},"desired":desired}
        ]}),
        Actor::Automatic,
    );
    let result = project(&captured, "apply", &request);
    assert_eq!(result["receipt"]["save"]["domain"]["outcome"], "superseded");
    assert_eq!(
        result["receipt"]["save"]["domain"]["reference_results"][0]["action"],
        "restore"
    );
    // A refused row still reports the value it would save, for a user to adopt.
    assert_eq!(
        result["receipt"]["save"]["domain"]["reference_results"][0]["desired"],
        desired
    );
    assert_eq!(records(&result["catalog"]), captured);
    let mut revisited = captured.clone();
    revisited["session"]["membership_epoch"] = json!(2);
    let result = project(&revisited, "apply", &request);
    assert_eq!(
        result["receipt"]["save"]["domain"]["conflicts"],
        json!(["inputs"])
    );
}

#[test]
fn enrollment_parser_and_constructors_share_typed_identity_and_naming_rules() {
    let mut wire = enrollment();
    wire["actor"] = json!("user");
    wire["stream"] = json!({"title": "New", "goal": "Goal", "goal_short": "Compact"});
    let parsed = EnrollmentRequest::parse(wire.clone(), "physical-source").unwrap();
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
    wire["stream"]["goal_short"] = json!("A short goal far over the limit");
    let long = EnrollmentRequest::parse(wire.clone(), "physical-source").unwrap();
    let stream = long.stream("host", "chosen-id").unwrap();
    assert_eq!(
        stream.goal_short.as_deref(),
        Some("A short goal far over th")
    );
    let session = parsed.session("host", "chosen-id").unwrap();
    assert_eq!(session.id, session_id_for("host", Harness::Pi, "native"));
    assert_eq!(session.stream_id, "chosen-id");
    assert_eq!(session.title_mode.to_string(), "manual");
    assert!(session.checkout.is_none());
    assert!(parsed.stream("", "chosen-id").is_err());
    assert!(parsed.session("host", "").is_err());

    wire["stream"] = json!({"title": "No goal"});
    let parsed = EnrollmentRequest::parse(wire.clone(), "physical-source").unwrap();
    assert_eq!(
        parsed.stream("host", "id").unwrap().goal_mode.to_string(),
        "automatic"
    );
    wire["stream"] = json!({"title": "Invalid", "goal_short": "Orphaned summary"});
    assert!(EnrollmentRequest::parse(wire.clone(), "physical-source").is_err());
}

#[test]
fn enrollment_rejects_native_actor_even_when_no_records_would_be_created() {
    let mut request = enrollment();
    request["actor"] = json!("native");
    request["stream_id"] = json!("effort");
    assert!(EnrollmentRequest::parse(request.clone(), "physical-source").is_err());
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
    request["harness_session_id"] = json!(native);
    request["stream"] = json!({"title": "New"});
    let result = project(&capture, "enroll", &request);
    let preview_name =
        r#"streams:preview:effort:["h\u00f8st", "pi", "n\u00e2tive/\ud83d\ude00\u007f"]"#;
    let expected_preview =
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, preview_name.as_bytes()).to_string();
    assert_eq!(result["catalog"]["stream"]["id"], expected_preview);
    assert_ne!(result["catalog"]["stream"]["id"], expected);
}

#[test]
fn enrollment_request_shape_and_text_validation_match_the_live_boundary() {
    let base = enrollment();
    for field in [
        "version",
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
            EnrollmentRequest::parse(request.clone(), "physical-source").is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("unexpected", json!(true)),
        ("version", json!(1)),
        ("version", json!("2")),
        ("authority", Value::Null),
        ("ownership_token", json!("old")),
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
            EnrollmentRequest::parse(request.clone(), "physical-source").is_err(),
            "{request}"
        );
        reject(&context(), "enroll", &request);
    }
    let mut explicit_null = base.clone();
    explicit_null["stream"] = Value::Null;
    explicit_null["stream_id"] = Value::Null;
    EnrollmentRequest::parse(explicit_null.clone(), "physical-source").unwrap();
    let duplicate = base.to_string().replacen(
        "\"title\":\"Contribution\"",
        "\"title\":\"Contribution\",\"title\":\"duplicate\"",
        1,
    );
    assert!(
        streams::codec::load_json(&duplicate)
            .and_then(|value| EnrollmentRequest::parse(value, "physical-source"))
            .is_err()
    );
}

#[test]
fn v2_enrollment_and_follow_on_edits_use_addressed_records_without_storage_or_authority() {
    let capture = empty();
    let original = capture.clone();
    let mut request = enrollment();
    request["stream"] = json!({"title":"New","goal":"Goal","goal_short":"Compact"});
    let first = project(&capture, "enroll", &request);
    assert_eq!(first, project(&capture, "enroll", &request));
    assert_eq!(capture, original);
    simulated(&first);
    simulated(&first["receipt"]);
    assert_eq!(first["receipt"]["saves"].as_array().unwrap().len(), 2);
    for save in first["receipt"]["saves"].as_array().unwrap() {
        simulated(&save["save"]);
    }
    assert!(first["catalog"].get("authority").is_none());
    let request = update(
        &first["catalog"],
        "session",
        json!({"state":"Follow-on","state_short":"Ready"}),
        Actor::Automatic,
    );
    let second = project(&first["catalog"], "apply", &request);
    assert_eq!(second["catalog"]["session"]["state_short"], "Ready");
    let request = update(
        &second["catalog"],
        "stream",
        json!({"goal":"Refined goal"}),
        Actor::Automatic,
    );
    let third = project(&second["catalog"], "apply", &request);
    assert_eq!(third["catalog"]["stream"]["goal"], "Refined goal");
    assert!(third["catalog"]["stream"]["goal_short"].is_null());
    assert_eq!(
        third["catalog"]["document_locations"],
        capture["document_locations"]
    );
}

#[test]
fn current_membership_history_is_preserved_without_loading_past_streams() {
    let mut capture = context();
    capture["session"]["membership_epoch"] = json!(3);
    let source = envelope(
        &capture,
        "apply",
        &update(
            &capture,
            "session",
            json!({"remaining":"Next"}),
            Actor::Automatic,
        ),
    )
    .to_string();
    let original = source.clone();
    let result = preview(&source).unwrap();
    assert_eq!(source, original);
    assert_eq!(result["catalog"]["session"]["remaining"], "Next");
    assert_eq!(result["catalog"]["session"]["membership_epoch"], json!(3));
    assert_eq!(result["catalog"]["checkouts"], capture["checkouts"]);
}

#[test]
fn manual_protection_derivative_clearing_and_inherited_defaults_are_domain_owned() {
    let mut capture = context();
    capture["title_defaults"]["stream"] = json!("manual");
    let result = project(
        &capture,
        "apply",
        &update(
            &capture,
            "stream",
            json!({"title":"Ignored"}),
            Actor::Automatic,
        ),
    );
    assert_eq!(result["catalog"]["stream"]["title"], "Effort");
    assert_eq!(result["receipt"]["save"]["domain"]["outcome"], "no-change");
    let result = project(
        &capture,
        "apply",
        &update(&capture, "stream", json!({"goal":null}), Actor::User),
    );
    assert!(result["catalog"]["stream"]["goal_short"].is_null());
    assert_eq!(result["catalog"]["stream"]["goal_mode"], "manual");
    let result = project(
        &capture,
        "apply",
        &update(
            &capture,
            "session",
            json!({"status":"done","state_short":"Finished"}),
            Actor::Automatic,
        ),
    );
    assert_eq!(result["catalog"]["session"]["state_short"], "Finished");
    let result = project(
        &capture,
        "apply",
        &update(&capture, "session", json!({"title":"Chosen"}), Actor::User),
    );
    assert_eq!(result["catalog"]["session"]["title_mode"], "manual");
}

#[test]
fn exact_before_source_and_automatic_stream_input_are_required_without_ownership_tokens() {
    let capture = context();
    let base = update(&capture, "session", json!({"state":"Next"}), Actor::User);
    for (field, value) in [
        ("catalog_source", json!("wrong")),
        ("ownership_token", json!("old")),
        ("version", json!(1)),
        ("title_default", Value::Null),
    ] {
        let mut request = base.clone();
        request[field] = value;
        reject(&capture, "apply", &request);
    }
    let mut request = base;
    request["before"]["state"] = json!("Stale");
    reject(&capture, "apply", &request);
    let mut request = update(&capture, "stream", json!({"goal":"Next"}), Actor::Automatic);
    request["inputs"][0]["record"]["state"] = json!("Stale");
    reject(&capture, "apply", &request);
    let mut request = update(
        &capture,
        "session",
        json!({"state":"Next"}),
        Actor::Automatic,
    );
    request["inputs"] = json!([{"kind":"stream","record":capture["stream"]}]);
    reject(&capture, "apply", &request);
    let mut request = update(&capture, "stream", json!({"goal":"Next"}), Actor::Automatic);
    request["inputs"] = json!([]);
    reject(&capture, "apply", &request);
    assert_eq!(capture, context());
}

#[test]
fn preview_does_not_invent_creation_retry_evidence_or_publication() {
    let mut request = enrollment();
    request["stream"] = json!({"title":"New"});
    let first = project(&empty(), "enroll", &request);
    reject(&first["catalog"], "enroll", &request);
    request.as_object_mut().unwrap().remove("stream");
    request["stream_id"] = first["catalog"]["stream"]["id"].clone();
    let repeated = project(&first["catalog"], "enroll", &request);
    assert_eq!(repeated["catalog"], first["catalog"]);
    assert_eq!(repeated["receipt"]["saves"].as_array().unwrap().len(), 1);
    assert_eq!(
        repeated["receipt"]["saves"][0]["save"]["domain"]["outcome"],
        "no-change"
    );
    simulated(&repeated["receipt"]);
    let mut request = enrollment();
    let mut uncarded = context();
    uncarded["session"] = Value::Null;
    reject(&uncarded, "enroll", &request);
    request["stream_id"] = json!("effort");
    assert!(project(&uncarded, "enroll", &request)["catalog"]["session"].is_object());
}

#[test]
fn foreign_records_are_not_preview_write_authority_and_enrollment_never_impersonates_a_session() {
    let mut capture = context();
    capture["stream"]["owner"]["host_id"] = json!("foreign");
    capture["session"]["host_id"] = json!("foreign");
    capture["session"]["id"] = json!(Session::id_for("foreign", "pi", "native"));
    let error = preview(
        &envelope(
            &capture,
            "apply",
            &update(&capture, "session", json!({"state":"Next"}), Actor::User),
        )
        .to_string(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("read-only"));
    for (field, value) in [
        ("stream_id", json!("other")),
        ("harness_session_id", json!("other-native")),
        ("harness", json!("codex")),
    ] {
        let mut request = enrollment();
        request[field] = value;
        reject(&context(), "enroll", &request);
    }
}

#[test]
fn context_fields_relationships_and_addressed_session_shape_are_strict() {
    let capture = context();
    let request = update(&capture, "session", json!({}), Actor::Automatic);
    for field in [
        "catalog_source",
        "host_id",
        "stream",
        "session",
        "title_defaults",
    ] {
        let mut broken = capture.clone();
        broken.as_object_mut().unwrap().remove(field);
        reject(&broken, "apply", &request);
    }
    let mut invalid = Vec::new();
    let mut broken = capture.clone();
    broken["authority"] = Value::Null;
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["sessions"] = json!([]);
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["session"]["stream_id"] = json!("elsewhere");
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["session"]["id"] = json!("legacy-native-alias");
    invalid.push(broken);
    let mut broken = capture.clone();
    broken["session"]["checkout"] =
        json!({"repository":"repo","ref":"main","directory_within_checkout":"."});
    invalid.push(broken);
    for broken in invalid {
        reject(&broken, "apply", &request);
    }
}

#[test]
fn malformed_objects_unknown_fields_enum_maps_and_duplicate_json_are_rejected() {
    let capture = context();
    let request = update(&capture, "stream", json!({"goal":"Next"}), Actor::Automatic);
    let base = envelope(&capture, "apply", &request);
    for path in [
        "/unexpected",
        "/catalog/unexpected",
        "/catalog/native",
        "/catalog/native_inventory",
        "/catalog/session/unexpected",
        "/catalog/stream/unexpected",
        "/request/unexpected",
    ] {
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
        "/request",
        "/request/before",
        "/request/inputs/0",
    ] {
        let mut wire = base.clone();
        *wire.pointer_mut(path).unwrap() = json!([]);
        assert!(preview(&wire.to_string()).is_err(), "{path}");
    }
    for (path, value) in [
        ("/action", json!({"apply":null})),
        ("/request/actor", json!({"automatic":null})),
        ("/request/kind", json!({"stream":null})),
        ("/catalog/session/status", json!({"active":null})),
        ("/catalog/stream/title_mode", json!({"inherit":null})),
    ] {
        let mut wire = base.clone();
        *wire.pointer_mut(path).unwrap() = value;
        assert!(preview(&wire.to_string()).is_err(), "{path}");
    }
    for source in ["null", "[]", "{}", r#"{"catalog":{},"catalog":{}}"#] {
        assert!(preview(source).is_err());
    }
    let source = base.to_string();
    let duplicate = source.replacen(
        "\"state\":\"Detailed state\"",
        "\"state\":\"Detailed state\",\"state\":\"duplicate\"",
        1,
    );
    assert_ne!(source, duplicate);
    assert!(preview(&duplicate).is_err());
}

#[test]
fn projected_catalog_carries_patch_schemas_for_its_new_records() {
    let captured = context();
    let automatic = |catalog: &Value| {
        catalog["patch_schemas"]["stream"]["automatic"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["field"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let request = update(
        &captured,
        "stream",
        json!({"title_mode":"manual"}),
        Actor::User,
    );
    let result = project(&captured, "apply", &request);
    assert_eq!(automatic(&result["catalog"]), ["goal", "goal_short"]);
    let unchanged = project(
        &captured,
        "apply",
        &update(
            &captured,
            "stream",
            json!({"title_mode":"automatic"}),
            Actor::User,
        ),
    );
    assert_eq!(
        automatic(&unchanged["catalog"]),
        ["title", "goal", "goal_short"]
    );
}
