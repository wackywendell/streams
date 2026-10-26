use super::*;
use crate::{
    agent::RecordContext,
    model::TitleDefaults,
    runtime::{
        conversations::IDENTITY_FILE_OPENS,
        native::PiCallerObservation,
        state::{NativePlacement, PlacementBinding, RuntimeState, herdr_scope},
    },
};

fn context() -> RecordContext {
    let session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "effort",
        "pi",
        "native",
        "Conversation",
    )
    .unwrap();
    RecordContext {
        catalog_source: "catalog".into(),
        host_id: "host".into(),
        title_defaults: TitleDefaults::default(),
        stream: Some(Stream::new("effort", "Effort", None, "host").unwrap()),
        session: Some(session.clone()),
        sessions: vec![session],
    }
}

fn inputs(f: &Fixture) -> (HerdrClient, RuntimeState, PathBuf) {
    let transcript = f._temp.path().join("native-owner.jsonl");
    std::fs::write(&transcript, "{\"type\":\"session\",\"id\":\"native\"}\n").unwrap();
    {
        let mut world = f.runner.world.lock().unwrap();
        world.snapshot["workspaces"][0]["tokens"] = json!({
            "strm_catalog":json_digest(&"catalog").unwrap(), "strm_host":"host",
            "strm_stream":"effort", "strm_binding":"binding"
        });
        let pane = &mut world.snapshot["panes"][0];
        pane["agent"] = json!("pi");
        pane["agent_session"] = json!({"agent":"pi","kind":"path","value":transcript});
        let owner = pane.clone();
        world.snapshot["agents"] = json!([owner]);
        world.observation_reads = 0;
        world.deny_read = true;
    }
    let client = HerdrClient::connect(f.environment.clone(), 1.0).unwrap();
    let runtime = RuntimeState::new(f._temp.path().join("runtime"), "host").unwrap();
    let scope = herdr_scope(&f.runner.socket, None);
    let cancelled = AtomicBool::new(false);
    let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
    runtime
        .save_placement_binding(
            &guard,
            context().stream.as_ref().unwrap(),
            None,
            &NativePlacement::new(
                "effort",
                &scope,
                PlacementBinding::herdr("w", "binding").unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    f.server.clear();
    IDENTITY_FILE_OPENS.with(|files| files.borrow_mut().clear());
    (client, runtime, transcript)
}

#[test]
fn record_native_pass_closes_without_source_context_proof_or_snapshot_rereads() {
    for malformed in [false, true] {
        let f = Fixture::new();
        f.engine().open_display(OpenMode::Explicit).unwrap();
        let ready = f.record();
        let (client, runtime, transcript) = inputs(&f);
        f.runner.world.lock().unwrap().ambiguous_close = malformed;
        let records = context();
        let cancelled = AtomicBool::new(false);
        let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
        let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot().unwrap()).unwrap();
        let caller = PiCallerObservation::from_snapshot(&client, snapshot, "native").unwrap();
        let membership = NativeMembershipObservation::observe(&records, &runtime, &caller).unwrap();
        let pass = NativePass::new(
            &records,
            membership,
            NativePorts {
                client: &client,
                writer: &guard,
                cancelled: &cancelled,
            },
        )
        .unwrap();
        assert!(std::ptr::eq(pass.records(), &records));
        assert!(std::ptr::eq(pass.caller(), &caller));
        assert_eq!(
            pass.scope().session_id,
            records.session.as_ref().unwrap().id
        );
        std::fs::rename(transcript, f._temp.path().join("old-transcript")).unwrap();
        std::fs::rename(runtime.root(), f._temp.path().join("old-runtime")).unwrap();
        assert_eq!(pass.close_display().is_err(), malformed);
        let closed = f.record();
        assert!(matches!(closed.phase(), NativePhase::Closed(_)));
        assert_eq!(closed.nonce(), ready.nonce());
        assert_eq!(closed.resources(), ready.resources());
        assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
        assert_eq!(
            f.server.methods(),
            ["session.snapshot", "plugin.pane.close"]
        );
        IDENTITY_FILE_OPENS.with(|files| assert_eq!(files.borrow().len(), 1));
    }
}

#[test]
fn record_native_pass_links_the_exact_receipt_with_the_original_proof() {
    let f = Fixture::new();
    f.runner.world.lock().unwrap().malformed_metadata_ack = true;
    assert!(f.engine().open_display(OpenMode::Explicit).is_err());
    let reserved = f.record();
    assert!(matches!(reserved.phase(), NativePhase::Reserved(_)));
    let (client, runtime, _) = inputs(&f);
    f.runner.world.lock().unwrap().malformed_metadata_ack = false;
    let records = context();
    let cancelled = AtomicBool::new(false);
    let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
    let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot().unwrap()).unwrap();
    let caller = PiCallerObservation::from_snapshot(&client, snapshot, "native").unwrap();
    let membership = NativeMembershipObservation::observe(&records, &runtime, &caller).unwrap();
    let pass = NativePass::new(
        &records,
        membership,
        NativePorts {
            client: &client,
            writer: &guard,
            cancelled: &cancelled,
        },
    )
    .unwrap();
    let resources = pass.link_reserved().unwrap();
    let ready = f.record();
    assert!(matches!(ready.phase(), NativePhase::Ready(_)));
    assert_eq!(ready.nonce(), reserved.nonce());
    assert_eq!(ready.resources(), Some(&resources));
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
    assert_eq!(
        f.server.methods(),
        ["session.snapshot", "pane.report_metadata"]
    );
    IDENTITY_FILE_OPENS.with(|files| assert_eq!(files.borrow().len(), 1));
}

#[test]
fn record_native_pass_rejects_wrong_context_before_native_effects() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let (client, runtime, _) = inputs(&f);
    let cancelled = AtomicBool::new(false);
    let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
    let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot().unwrap()).unwrap();
    let caller = PiCallerObservation::from_snapshot(&client, snapshot, "native").unwrap();
    let changes: [fn(&mut RecordContext); 11] = [
        |records| records.catalog_source.push('x'),
        |records| records.host_id.push('x'),
        |records| records.stream = None,
        |records| records.session = None,
        |records| records.session.as_mut().unwrap().id.push('x'),
        |records| records.session.as_mut().unwrap().host_id.push('x'),
        |records| records.stream.as_mut().unwrap().owner_host_id.push('x'),
        |records| records.session.as_mut().unwrap().stream_id.push('x'),
        |records| records.stream.as_mut().unwrap().id.push('x'),
        |records| {
            let session = records.session.as_mut().unwrap();
            session.harness_session_id.push('x');
            session.id = Session::id_for(
                &session.host_id,
                &session.harness,
                &session.harness_session_id,
            );
        },
        |records| {
            let session = records.session.as_mut().unwrap();
            session.harness = "codex".into();
            session.id = Session::id_for(
                &session.host_id,
                &session.harness,
                &session.harness_session_id,
            );
        },
    ];
    for change in changes {
        let original = context();
        let mut records = original.clone();
        change(&mut records);
        let membership =
            NativeMembershipObservation::observe(&original, &runtime, &caller).unwrap();
        assert!(
            NativePass::new(
                &records,
                membership,
                NativePorts {
                    client: &client,
                    writer: &guard,
                    cancelled: &cancelled
                }
            )
            .is_err()
        );
    }
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
}

#[test]
fn canceled_record_native_pass_does_not_publish_closed_intent() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let before = f.record();
    let (client, runtime, _) = inputs(&f);
    let records = context();
    let cancelled = AtomicBool::new(false);
    let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
    let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot().unwrap()).unwrap();
    let caller = PiCallerObservation::from_snapshot(&client, snapshot, "native").unwrap();
    let membership = NativeMembershipObservation::observe(&records, &runtime, &caller).unwrap();
    let pass = NativePass::new(
        &records,
        membership,
        NativePorts {
            client: &client,
            writer: &guard,
            cancelled: &cancelled,
        },
    )
    .unwrap();
    cancelled.store(true, Ordering::Release);
    assert!(pass.close_display().is_err());
    assert_eq!(f.record(), before);
    assert_eq!(f.server.methods(), ["session.snapshot"]);
}

#[test]
fn incomplete_binding_refuses_before_private_evidence_read() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let (client, runtime, _) = inputs(&f);
    f.runner.world.lock().unwrap().snapshot["workspaces"][0]["tokens"]
        .as_object_mut()
        .unwrap()
        .remove("strm_binding");
    let records = context();
    let cancelled = AtomicBool::new(false);
    let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
    std::fs::write(
        f.runner.state.path(&f.runner.key).unwrap(),
        "corrupt evidence",
    )
    .unwrap();
    let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot().unwrap()).unwrap();
    let caller = PiCallerObservation::from_snapshot(&client, snapshot, "native").unwrap();
    let membership = NativeMembershipObservation::observe(&records, &runtime, &caller).unwrap();
    assert!(matches!(
        membership.plan().unwrap(),
        NativeMembershipPlan::NeedsProjection { .. }
    ));
    let error = NativePass::new(
        &records,
        membership,
        NativePorts {
            client: &client,
            writer: &guard,
            cancelled: &cancelled,
        },
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("complete destination placement"), "{error}");
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
}

#[test]
fn record_native_pass_rejects_wrong_evidence_purpose_or_endpoint() {
    for (field, value) in [("purpose", "editor"), ("endpoint", "/another.sock")] {
        let f = Fixture::new();
        f.engine().open_display(OpenMode::Explicit).unwrap();
        let mut record = serde_json::to_value(f.record()).unwrap();
        record[field] = json!(value);
        std::fs::write(
            f.runner.state.path(&f.runner.key).unwrap(),
            record.to_string(),
        )
        .unwrap();
        let (client, runtime, _) = inputs(&f);
        let records = context();
        let cancelled = AtomicBool::new(false);
        let guard = f.runner.state.lock(1.0, &cancelled).unwrap();
        let snapshot = HerdrNativeSnapshot::parse(&client.session_snapshot().unwrap()).unwrap();
        let caller = PiCallerObservation::from_snapshot(&client, snapshot, "native").unwrap();
        let membership = NativeMembershipObservation::observe(&records, &runtime, &caller).unwrap();
        assert!(
            NativePass::new(
                &records,
                membership,
                NativePorts {
                    client: &client,
                    writer: &guard,
                    cancelled: &cancelled,
                }
            )
            .is_err()
        );
        assert_eq!(f.server.methods(), ["session.snapshot"]);
        assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
    }
}
