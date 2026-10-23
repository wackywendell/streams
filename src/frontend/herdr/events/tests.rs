use super::*;
use crate::frontend::herdr::socket::tests::{ping, request, server};
use std::io::Write;
use std::thread;

fn filter(kinds: &[EventKind]) -> EventFilter {
    EventFilter {
        kinds: kinds.iter().copied().collect(),
        pane_ids: BTreeSet::from(["owner".into(), "card".into()]),
        tab_ids: BTreeSet::from(["tab".into()]),
        workspace_ids: BTreeSet::from(["workspace".into()]),
        ..EventFilter::default()
    }
}

#[test]
fn all_required_event_kinds_and_old_pane_id_match() {
    let cases = [
        (
            EventKind::PaneMoved,
            json!({"type":"pane_moved","previous_pane_id":"owner","previous_tab_id":"tab","previous_workspace_id":"workspace","pane":{"pane_id":"new","tab_id":"new-tab","workspace_id":"new-workspace"}}),
        ),
        (
            EventKind::TabMoved,
            json!({"type":"tab_moved","tab_id":"tab","workspace_id":"other","insert_index":0,"tabs":[]}),
        ),
        (
            EventKind::PaneClosed,
            json!({"type":"pane_closed","pane_id":"owner","workspace_id":"workspace"}),
        ),
        (
            EventKind::PaneExited,
            json!({"type":"pane_exited","pane_id":"owner","workspace_id":"workspace"}),
        ),
        (
            EventKind::TabClosed,
            json!({"type":"tab_closed","tab_id":"tab","workspace_id":"workspace"}),
        ),
        (
            EventKind::WorkspaceClosed,
            json!({"type":"workspace_closed","workspace_id":"workspace"}),
        ),
        (
            EventKind::PaneUpdated,
            json!({"type":"pane_updated","pane":{"pane_id":"card","tab_id":"tab","workspace_id":"workspace"}}),
        ),
        (
            EventKind::PaneAgentDetected,
            json!({"type":"pane_agent_detected","pane_id":"owner","workspace_id":"workspace"}),
        ),
        (
            EventKind::WorkspaceMetadataUpdated,
            json!({"type":"workspace_metadata_updated","workspace":{"workspace_id":"workspace"}}),
        ),
    ];
    let schema = crate::frontend::herdr::socket::tests::schema();
    assert_eq!(cases.len(), schema["events"].as_array().unwrap().len());
    let mut seen = BTreeSet::new();
    for (kind, data) in cases {
        assert!(seen.insert(kind));
        let entry = schema["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["properties"]["type"]["const"] == data["type"])
            .unwrap();
        for field in entry["required"].as_array().unwrap() {
            assert!(
                data.get(field.as_str().unwrap()).is_some(),
                "{data}: missing {field}"
            );
        }
        for path in [
            "/pane_id",
            "/tab_id",
            "/workspace_id",
            "/previous_pane_id",
            "/previous_tab_id",
            "/previous_workspace_id",
            "/pane/pane_id",
            "/pane/tab_id",
            "/pane/workspace_id",
            "/workspace/workspace_id",
        ] {
            if data.pointer(path).is_none() {
                continue;
            }
            for invalid in ["", " ", "unrelated\n"] {
                let mut changed = data.clone();
                *changed.pointer_mut(path).unwrap() = json!(invalid);
                let bytes =
                    serde_json::to_vec(&json!({"event":data["type"],"data":changed})).unwrap();
                assert!(
                    matches!(
                        filter(&[kind]).decode(&bytes),
                        Err(SubscriptionError::InvalidEvent(_))
                    ),
                    "{changed}"
                );
            }
        }
        let envelope = json!({"event":data["type"],"data":data});
        let event = filter(&[kind])
            .decode(&serde_json::to_vec(&envelope).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(EventKind::from(&event), kind);
        assert_eq!(kind.to_string().parse::<EventKind>().unwrap(), kind);
    }
}

#[test]
fn early_id_filter_skips_unrelated_payload_before_typed_decode() {
    // tab/workspace are invalid for a typed EventPane but irrelevant IDs never
    // reach that decoder. Unknown output types never decode even the header.
    let f = filter(&[EventKind::PaneUpdated]);
    let unrelated = br#"{"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"unrelated"},"expensive":{"arbitrary":[1,2,3]}}}"#;
    assert!(f.decode(unrelated).unwrap().is_none());
    assert!(
        f.decode(br#"{"event":"pane_output_changed","data":42}"#)
            .unwrap()
            .is_none()
    );
    assert!(f.decode(br#"{"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"owner"}}}"#).is_err());
}

#[test]
fn subscribes_by_type_before_first_read_preserves_buffered_events_and_resubscribes_once() {
    let (_directory, listener, path) = server();
    let handle = thread::spawn(move || {
        ping(&listener);
        // Check subscribe-time filtering, not just client-side filtering.
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["method"], "events.subscribe");
        assert_eq!(
            value["params"],
            json!({"subscriptions":[{"type":"pane.closed"}]})
        );
        // Coalesced ack and event exercise retention of BufReader's queue.
        writeln!(reader.get_mut(), "{}\n{}", json!({"id":value["id"],"result":{"type":"subscription_started"}}),
            json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"owner","workspace_id":"workspace"}})).unwrap();
        drop(reader);
        let mut stream = request(
            &listener,
            "events.subscribe",
            json!({"type":"subscription_started"}),
        );
        writeln!(stream.get_mut(), "{}", json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":"card","workspace_id":"workspace"}})).unwrap();
        drop(stream);
        // Even successful resubscription spends the sole reconnect allowance.
        listener.set_nonblocking(true).unwrap();
        assert!(listener.accept().is_err());
    });
    let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
    let mut subscription = client.subscribe(filter(&[EventKind::PaneClosed])).unwrap();
    let first = subscription.receive().unwrap();
    assert!(
        matches!(&first, SubscriptionUpdate::Event(HerdrEvent::PaneClosed { pane_id, .. }) if pane_id == "owner"),
        "{first:?}"
    );
    assert_eq!(
        subscription.receive().unwrap(),
        SubscriptionUpdate::Resubscribed
    );
    assert!(
        matches!(subscription.receive().unwrap(), SubscriptionUpdate::Event(HerdrEvent::PaneClosed { pane_id, .. }) if pane_id == "card")
    );
    assert!(matches!(
        subscription.receive(),
        Err(SubscriptionError::Resubscribe(_))
    ));
    assert!(matches!(
        subscription.receive(),
        Err(SubscriptionError::Stopped)
    ));
    handle.join().unwrap();
}

#[test]
fn malformed_routing_ids_fail_closed_even_when_unrelated() {
    for (kind, tag, data) in [
        (
            EventKind::PaneMoved,
            "pane_moved",
            json!({"previous_pane_id":"unrelated","pane":{}}),
        ),
        (
            EventKind::PaneMoved,
            "pane_moved",
            json!({"pane":{"pane_id":"unrelated"}}),
        ),
        (EventKind::PaneUpdated, "pane_updated", json!({"pane":{}})),
        (EventKind::PaneClosed, "pane_closed", json!({})),
        (EventKind::PaneExited, "pane_exited", json!({})),
        (
            EventKind::PaneAgentDetected,
            "pane_agent_detected",
            json!({}),
        ),
        (EventKind::TabMoved, "tab_moved", json!({})),
        (EventKind::TabClosed, "tab_closed", json!({})),
        (EventKind::WorkspaceClosed, "workspace_closed", json!({})),
        (
            EventKind::WorkspaceMetadataUpdated,
            "workspace_metadata_updated",
            json!({"workspace":{}}),
        ),
    ] {
        for null in [false, true] {
            let mut data = data.clone();
            data["type"] = json!(tag);
            if null {
                for key in ["pane_id", "tab_id", "workspace_id", "previous_pane_id"] {
                    if data.get(key).is_none() {
                        data[key] = serde_json::Value::Null;
                    }
                }
            }
            let bytes = serde_json::to_vec(&json!({"event":tag,"data":data})).unwrap();
            assert!(
                matches!(
                    filter(&[kind]).decode(&bytes),
                    Err(SubscriptionError::InvalidEvent(_))
                ),
                "{data}"
            );
        }
    }
}

#[test]
fn escaped_tags_and_ids_decode_but_dotted_wire_tags_are_not_topics() {
    let f = filter(&[EventKind::PaneClosed]);
    let bytes = br#"{"event":"pane_\u0063losed","data":{"type":"pane_\u0063losed","pane_id":"\u006fwner","workspace_id":"workspace"}}"#;
    assert!(
        matches!(f.decode(bytes).unwrap(), Some(HerdrEvent::PaneClosed { pane_id, .. }) if pane_id == "owner")
    );
    assert!(matches!(
        f.decode(br#"{"event":"pane_\u0063losed","data":{"type":"pane_closed","pane_id":"unrelated\n"}}"#),
        Err(SubscriptionError::InvalidEvent(_))
    ));
    assert!(
        f.decode(br#"{"event":"pane.closed","data":42}"#)
            .unwrap()
            .is_none()
    );
}

#[test]
fn watched_ids_change_without_another_subscribe() {
    let (_directory, listener, path) = server();
    let handle = thread::spawn(move || {
        ping(&listener);
        let mut stream = request(
            &listener,
            "events.subscribe",
            json!({"type":"subscription_started"}),
        );
        for pane in ["owner", "moved-owner"] {
            writeln!(stream.get_mut(), "{}", json!({"event":"pane_closed","data":{"type":"pane_closed","pane_id":pane,"workspace_id":"workspace"}})).unwrap();
        }
    });
    let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
    let mut subscription = client.subscribe(filter(&[EventKind::PaneClosed])).unwrap();
    subscription.set_watched_ids(
        BTreeSet::from(["moved-owner".into()]),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    assert!(
        matches!(subscription.receive().unwrap(), SubscriptionUpdate::Event(HerdrEvent::PaneClosed { pane_id, .. }) if pane_id == "moved-owner")
    );
    handle.join().unwrap();
}

#[test]
fn cancellation_handles_follow_the_active_socket() {
    for reconnect in [false, true] {
        let (_directory, listener, path) = server();
        let server = thread::spawn(move || {
            ping(&listener);
            let mut reader = request(
                &listener,
                "events.subscribe",
                json!({"type":"subscription_started"}),
            );
            if reconnect {
                drop(reader);
                reader = request(
                    &listener,
                    "events.subscribe",
                    json!({"type":"subscription_started"}),
                );
            }
            // Remain connected until cancellation closes the client side.
            assert!(
                read_frame(
                    &mut reader,
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(2))
                )
                .is_err()
            );
        });
        let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
        let mut subscription = client.subscribe(filter(&[EventKind::PaneClosed])).unwrap();
        let before = subscription.cancellation_handle();
        if reconnect {
            assert_eq!(
                subscription.receive().unwrap(),
                SubscriptionUpdate::Resubscribed
            );
        }
        let after = subscription.cancellation_handle();
        let receiver = thread::spawn(move || subscription.receive());
        before.cancel().unwrap();
        assert!(matches!(
            receiver.join().unwrap(),
            Err(SubscriptionError::Stopped)
        ));
        // Both old and newly acquired handles reference the same cancellation.
        after.cancel().unwrap();
        server.join().unwrap();
    }
}

#[test]
fn cancellation_during_resubscribe_cannot_install_a_live_reader() {
    let (_directory, listener, path) = server();
    let (pending_tx, pending_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        ping(&listener);
        drop(request(
            &listener,
            "events.subscribe",
            json!({"type":"subscription_started"}),
        ));
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let bytes = read_frame(&mut reader, None).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        pending_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        writeln!(
            reader.get_mut(),
            "{}",
            json!({"id":request["id"],"result":{"type":"subscription_started"}})
        )
        .unwrap();
        assert!(
            read_frame(
                &mut reader,
                Some(std::time::Instant::now() + std::time::Duration::from_secs(2))
            )
            .is_err()
        );
    });
    let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
    let mut subscription = client.subscribe(filter(&[EventKind::PaneClosed])).unwrap();
    let cancel = subscription.cancellation_handle();
    let receiver = thread::spawn(move || subscription.receive());
    pending_rx.recv().unwrap();
    let _ = cancel.cancel();
    release_tx.send(()).unwrap();
    assert!(matches!(
        receiver.join().unwrap(),
        Err(SubscriptionError::Stopped)
    ));
    server.join().unwrap();
}

#[test]
fn oversized_frames_are_invalid_events_not_disconnects() {
    let (_directory, listener, path) = server();
    let server = thread::spawn(move || {
        ping(&listener);
        let mut reader = request(
            &listener,
            "events.subscribe",
            json!({"type":"subscription_started"}),
        );
        let _ = reader
            .get_mut()
            .write_all(&vec![b'x'; 16 * 1024 * 1024 + 1]);
    });
    let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
    let mut subscription = client.subscribe(filter(&[EventKind::PaneClosed])).unwrap();
    assert!(matches!(
        subscription.receive(),
        Err(SubscriptionError::InvalidEvent(_))
    ));
    assert!(matches!(
        subscription.receive(),
        Err(SubscriptionError::Stopped)
    ));
    drop(subscription);
    server.join().unwrap();
}

#[test]
fn missing_subscribe_support_is_a_typed_startup_error() {
    let (_directory, listener, path) = server();
    let handle = thread::spawn(move || {
        ping(&listener);
        request(&listener, "events.subscribe", json!({"type":"unsupported"}));
    });
    let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
    assert!(matches!(
        client.subscribe(filter(&[EventKind::PaneMoved])),
        Err(SubscriptionError::Establish(_))
    ));
    handle.join().unwrap();
}
