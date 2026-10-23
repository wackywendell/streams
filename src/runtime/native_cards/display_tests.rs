use super::*;
use crate::frontend::herdr::events::{EventFilter, EventKind};

fn observe(
    f: &Fixture,
    lifetime: &mut DisplayLifetime,
    client: &HerdrClient,
) -> Result<CardSnapshot> {
    let evidence = lifetime
        .read_evidence()?
        .ok_or_else(|| invalid("card lifetime ended"))?;
    let native = HerdrNativeSnapshot::parse(&client.session_snapshot()?)?;
    let placement = lifetime
        .observe_native(client, &native, &evidence)?
        .ok_or_else(|| invalid("owner closed"))?;
    let mut source = Source {
        world: f.runner.world.clone(),
        launch: SourceLaunch {
            environment: f.environment.clone(),
            cwd: f._temp.path().canonicalize().unwrap(),
        },
    };
    let observed = source.read(client, &native, false)?;
    lifetime.admit(&observed.card, &placement)?;
    Ok(observed.card)
}
#[test]
fn startup_first_paint_uses_one_observation_instead_of_two_and_adds_only_the_required_subscription()
{
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let environment = f.launched_environment();
    f.server.clear();
    f.runner.world.lock().unwrap().observation_reads = 0;
    let old = f.engine_with(environment.clone()).verify_display().unwrap();
    let mut view = CardView::new();
    let token = view.begin_refresh();
    assert!(view.complete_refresh(token, Ok(old)));
    view.paint(Rect::new(0, 0, 100, 20));
    assert_eq!(f.server.methods(), ["ping", "session.snapshot"]);
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 2);
    let evidence = f.record();

    f.server.clear();
    f.runner.world.lock().unwrap().observation_reads = 0;
    let client = HerdrClient::connect(environment.clone(), 1.0).unwrap();
    let _subscription = client
        .socket()
        .subscribe(EventFilter {
            kinds: [EventKind::PaneMoved].into(),
            pane_ids: ["owner".into(), "display".into()].into(),
            tab_ids: ["t".into()].into(),
            workspace_ids: ["w".into()].into(),
            ..EventFilter::default()
        })
        .unwrap();
    let context = LaunchedCardContext::from_environment(&environment)
        .unwrap()
        .unwrap();
    let mut lifetime = DisplayLifetime::new(context, f.runner.socket.clone()).unwrap();
    let frame = observe(&f, &mut lifetime, &client).unwrap();
    let token = view.begin_refresh();
    assert!(view.complete_refresh(token, Ok(frame)));
    view.paint(Rect::new(0, 0, 100, 20));
    assert_eq!(
        f.server.methods(),
        ["ping", "events.subscribe", "session.snapshot"]
    );
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 1);
    assert_eq!(
        f.record(),
        evidence,
        "painting never transitions native evidence"
    );
}
#[test]
fn first_read_paints_before_pair_publication_without_waiting_or_resolving_launch_uncertainty() {
    for unknown_acknowledgement in [false, true] {
        let f = Fixture::new();
        if unknown_acknowledgement {
            f.runner.world.lock().unwrap().malformed_open = true;
            assert!(f.engine().open_display(OpenMode::Explicit).is_err());
        } else {
            f.reserve(false);
        }
        let environment = f.launched_environment();
        let error = f
            .engine_with(environment.clone())
            .verify_display()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("caller is not the exact owned card"),
            "{error}"
        );
        let before = f.record();
        assert!(matches!(
            before.phase(),
            NativePhase::Unlaunched | NativePhase::Reserved(_)
        ));
        f.server.clear();
        f.runner.world.lock().unwrap().observation_reads = 0;
        let context = LaunchedCardContext::from_environment(&environment)
            .unwrap()
            .unwrap();
        let mut lifetime = DisplayLifetime::new(context, f.runner.socket.clone()).unwrap();
        let client = HerdrClient::connect(environment, 1.0).unwrap();
        assert!(observe(&f, &mut lifetime, &client).is_ok());
        assert_eq!(f.record(), before);
        assert_eq!(f.runner.world.lock().unwrap().observation_reads, 1);
        assert_eq!(f.server.methods(), ["ping", "session.snapshot"]);
    }
}
#[test]
fn display_follows_owner_and_membership_while_separated_effects_still_refuse() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let environment = f.launched_environment();
    let context = LaunchedCardContext::from_environment(&environment)
        .unwrap()
        .unwrap();
    let mut lifetime = DisplayLifetime::new(context.clone(), f.runner.socket.clone()).unwrap();
    let client = HerdrClient::connect(environment.clone(), 1.0).unwrap();
    observe(&f, &mut lifetime, &client).unwrap();
    let before = f.record();
    {
        let mut world = f.runner.world.lock().unwrap();
        world.snapshot["panes"][0]["pane_id"] = json!("owner-next");
        world.snapshot["panes"][0]["tab_id"] = json!("next-tab");
        world.snapshot["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"workspace_id":"w","tab_id":"next-tab","label":"2","number":2}));
        world.snapshot["layouts"][0]["panes"] = json!([{"pane_id":"display"}]);
        world.snapshot["layouts"].as_array_mut().unwrap().push(
            json!({"workspace_id":"w","tab_id":"next-tab","panes":[{"pane_id":"owner-next"}]}),
        );
        let stream = Stream::new("destination", "Destination", None, "host").unwrap();
        let mut session = world.observation.card.session().clone();
        session.stream_id = stream.id.clone();
        session.id = "migrated-session-id".into();
        world.observation.card = CardSnapshot::new(
            stream,
            session,
            DisplayContext {
                catalog_source: "catalog".into(),
                owner: NativeOwnerObservation {
                    pane_id: "owner-next".into(),
                    tab_id: "next-tab".into(),
                    workspace_id: "w".into(),
                },
                checkouts: vec![],
                documents: vec![],
            },
        )
        .unwrap();
    }
    f.server.clear();
    f.runner.world.lock().unwrap().observation_reads = 0;
    let moved = observe(&f, &mut lifetime, &client).unwrap();
    assert_eq!(moved.scope().stream_id, "destination");
    assert_eq!(moved.owner().pane_id, "owner-next");
    assert_eq!(
        lifetime.context().scope(),
        context.scope(),
        "launch context remains an immutable lifetime restriction"
    );
    assert_eq!(f.record(), before);
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 1);
    let error = f
        .engine_with(environment)
        .verify_display()
        .unwrap_err()
        .to_string();
    assert!(error.contains("outside this card workspace/tab"), "{error}");
}
#[test]
fn missing_or_replaced_evidence_retires_the_card_without_mutating_the_replacement() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let environment = f.launched_environment();
    let context = LaunchedCardContext::from_environment(&environment)
        .unwrap()
        .unwrap();
    let lifetime = DisplayLifetime::new(context, f.runner.socket.clone()).unwrap();
    assert!(lifetime.read_evidence().unwrap().is_some());
    let old = f.record();
    let replacement = NativeAttempt::new(old.key(), "replacement").unwrap();
    f.write(Some(&old), &replacement);
    f.server.clear();
    assert!(lifetime.read_evidence().unwrap().is_none());
    assert_eq!(f.record(), replacement);
    {
        let lock = f
            .runner
            .state
            .lock(&f.runner.socket, "w", 1.0, &AtomicBool::new(false))
            .unwrap();
        lock.discard_unlaunched(&replacement, None).unwrap();
    }
    assert!(lifetime.read_evidence().unwrap().is_none());
    assert!(f.runner.state.read(replacement.key()).unwrap().is_none());
    assert!(f.server.methods().is_empty());
}
#[test]
fn complete_native_observation_of_a_missing_owner_requires_no_source_read_or_writes() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let environment = f.launched_environment();
    let context = LaunchedCardContext::from_environment(&environment)
        .unwrap()
        .unwrap();
    let mut lifetime = DisplayLifetime::new(context, f.runner.socket.clone()).unwrap();
    let client = HerdrClient::connect(environment, 1.0).unwrap();
    observe(&f, &mut lifetime, &client).unwrap();
    let before = f.record();
    let mut raw = f.runner.world.lock().unwrap().snapshot.clone();
    raw["panes"]
        .as_array_mut()
        .unwrap()
        .retain(|pane| pane["pane_id"] != "owner");
    raw["layouts"][0]["panes"] = json!([{"pane_id":"display"}]);
    let snapshot = HerdrNativeSnapshot::parse(&raw).unwrap();
    f.server.clear();
    f.runner.world.lock().unwrap().observation_reads = 0;
    assert!(
        lifetime
            .observe_native(&client, &snapshot, &before)
            .unwrap()
            .is_none()
    );
    assert!(f.server.methods().is_empty());
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
    assert_eq!(f.record(), before);
}
