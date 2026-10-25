use super::*;
use crate::frontend::herdr::{
    HerdrClient,
    socket::tests::{ping, request, server},
};
use std::{collections::BTreeMap, io::Write, sync::mpsc, thread};

#[test]
fn a_foreground_pass_observes_disconnect_without_reconnecting_or_touching_reused_resources() {
    use crate::fake_herdr::{FakeHerdr, Reply};
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let resources = Arc::new(Mutex::new(
        json!([{"pane_id":"w:p","resource":"original card","closed":false,"moved":false}]),
    ));
    let shared = resources.clone();
    let fake = FakeHerdr::start(
        &directory.path().join("herdr.sock"),
        move |method, _| match method {
            "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
            "session.snapshot" => Reply::Result(
                json!({"type":"session_snapshot","snapshot":{"panes":shared.lock().unwrap().clone()}}),
            ),
            "pane.close" | "pane.move" => {
                shared.lock().unwrap()[0][if method == "pane.close" {
                    "closed"
                } else {
                    "moved"
                }] = json!(true);
                Reply::Result(json!({"type":"ok"}))
            }
            other => panic!("unexpected method {other}"),
        },
    );
    let client = HerdrClient::connect(
        BTreeMap::from([(
            "HERDR_SOCKET_PATH".into(),
            fake.socket.to_str().unwrap().into(),
        )]),
        1.0,
    )
    .unwrap()
    .observing()
    .unwrap();
    let snapshot = client.session_snapshot().unwrap();
    resources.lock().unwrap()[0]["resource"] = json!("unrelated replacement");
    let before = resources.lock().unwrap().clone();
    fake.disconnect_subscriptions();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !client
        .subscription_ended
        .as_ref()
        .unwrap()
        .load(Ordering::Acquire)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "disconnect was not observed"
        );
        thread::sleep(std::time::Duration::from_millis(1));
    }
    for method in ["pane.close", "pane.move"] {
        assert!(
            client
                .observing()
                .unwrap()
                .card_request(
                    method,
                    json!({"pane_id":snapshot["panes"][0]["pane_id"]}),
                    None,
                    true
                )
                .is_err()
        );
    }
    assert_eq!(*resources.lock().unwrap(), before);
    assert_eq!(
        fake.methods(),
        ["ping", "events.subscribe", "session.snapshot"]
    );
}

#[test]
fn a_restarted_server_cannot_revive_an_observed_pass_or_touch_an_unrelated_pane() {
    let (_directory, listener, path) = server();
    let server_path = path.clone();
    let (restart_tx, restart_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let fake = thread::spawn(move || {
        ping(&listener);
        let old_subscription = request(
            &listener,
            "events.subscribe",
            json!({"type":"subscription_started"}),
        );
        drop(request(
            &listener,
            "session.snapshot",
            json!({"type":"session_snapshot","snapshot":{"panes":[{
                "pane_id":"w2:p1","identity":"original-card",
                "tokens":{"strm_role":"card","strm_session":"session"}
            }]}}),
        ));
        restart_rx.recv().unwrap();
        drop(old_subscription);
        drop(listener);
        std::fs::remove_file(&server_path).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&server_path).unwrap();
        let mut resources = json!([
            {"pane_id":"w2:p1","identity":"unrelated-pane",
             "tokens":{"strm_role":"card","strm_session":"other"},
             "closed":false,"moved":false},
            {"pane_id":"w1:p1","identity":"unrelated-destination",
             "tokens":{},"closed":false,"moved":false},
            {"pane_id":"w2:p2","identity":"new-session-card",
             "tokens":{"strm_role":"card","strm_session":"session"},
             "closed":false,"moved":false}
        ]);
        ready_tx.send(()).unwrap();
        let mut subscription = None;
        loop {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let frame = read_frame(&mut reader, None).unwrap();
            let request: serde_json::Value = serde_json::from_slice(&frame).unwrap();
            let result = match request["method"].as_str().unwrap() {
                "events.subscribe" => {
                    writeln!(
                        reader.get_mut(),
                        "{}",
                        json!({
                            "id":request["id"],"result":{"type":"subscription_started"}
                        })
                    )
                    .unwrap();
                    subscription = Some(reader);
                    continue;
                }
                "session.snapshot" => json!({
                    "type":"session_snapshot","snapshot":{"panes":resources}
                }),
                method @ ("pane.close" | "pane.move") => {
                    let resource = resources
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|pane| pane["pane_id"] == request["params"]["pane_id"])
                        .unwrap();
                    resource[if method == "pane.close" {
                        "closed"
                    } else {
                        "moved"
                    }] = json!(true);
                    json!({"type":"effect_applied"})
                }
                "finish" => break,
                method => panic!("unexpected method: {method}"),
            };
            writeln!(
                reader.get_mut(),
                "{}",
                json!({"id":request["id"],"result":result})
            )
            .unwrap();
        }
        drop(subscription);
        resources
    });
    let client = HerdrClient::connect(
        BTreeMap::from([("HERDR_SOCKET_PATH".into(), path.to_str().unwrap().into())]),
        1.0,
    )
    .unwrap();
    let mut subscription = client
        .socket()
        .subscribe(EventFilter {
            kinds: BTreeSet::from([EventKind::PaneClosed]),
            ..EventFilter::default()
        })
        .unwrap();
    let cancel = subscription.cancellation_handle();
    let old_ended = cancel.ended();
    let pass = client.with_subscription(&cancel);
    let observed = pass.session_snapshot().unwrap();
    let card = observed["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| {
            pane["tokens"]["strm_role"] == "card" && pane["tokens"]["strm_session"] == "session"
        })
        .unwrap();
    restart_tx.send(()).unwrap();
    ready_rx.recv().unwrap();
    assert_eq!(
        subscription.receive().unwrap(),
        SubscriptionUpdate::Resubscribed
    );
    assert!(old_ended.load(Ordering::Acquire));
    assert!(!cancel.ended().load(Ordering::Acquire));
    for (method, params) in [
        ("pane.close", json!({"pane_id":card["pane_id"]})),
        (
            "pane.move",
            json!({"pane_id":card["pane_id"],"destination":{
                "type":"existing_tab","tab_id":"w1:t1","target_pane_id":"w1:p1","split":"down"
            }}),
        ),
    ] {
        let error = pass.card_request(method, params, None, true).unwrap_err();
        assert!(error.to_string().contains("subscription ended"), "{error}");
    }
    // A later pass has a new observation and can act on the current Session's card.
    let fresh = client.with_subscription(&cancel);
    let observed = fresh.session_snapshot().unwrap();
    let card = observed["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pane| {
            pane["tokens"]["strm_role"] == "card" && pane["tokens"]["strm_session"] == "session"
        })
        .unwrap();
    fresh
        .card_request("pane.close", json!({"pane_id":card["pane_id"]}), None, true)
        .unwrap();
    let mut finish = UnixStream::connect(path).unwrap();
    writeln!(finish, "{}", json!({"id":"finish","method":"finish"})).unwrap();
    let resources = fake.join().unwrap();
    for pane in resources.as_array().unwrap() {
        if pane["tokens"]["strm_session"] != "session" {
            assert_eq!(pane["closed"], false, "{pane}");
            assert_eq!(pane["moved"], false, "{pane}");
        }
    }
    assert_eq!(resources[2]["closed"], true);
    drop(subscription);
    assert!(cancel.ended().load(Ordering::Acquire));
}
