use super::*;
use crate::fake_herdr::{FakeHerdr, Reply};
use serde_json::json;
use std::{collections::BTreeMap, sync::mpsc, time::Duration};

fn fixture() -> (
    tempfile::TempDir,
    FakeHerdr,
    HerdrClient,
    Events,
    mpsc::Receiver<PaneEvent>,
) {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let socket = directory.path().join("herdr.sock");
    let server = FakeHerdr::start(&socket, |method, _| match method {
        "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
        "session.snapshot" => Reply::Result(json!({"type":"session_snapshot","snapshot":{}})),
        other => panic!("unexpected card request: {other}"),
    });
    let environment = BTreeMap::from([
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "card".into()),
        ("HERDR_SOCKET_PATH".into(), socket.to_str().unwrap().into()),
    ]);
    let client = HerdrClient::connect(environment, 1.0).unwrap();
    let (sender, receiver) = mpsc::channel();
    let events = Events::new(PaneEvents::new(move |event| {
        let _ = sender.send(event);
    }));
    (directory, server, client, events, receiver)
}
fn pane(id: &str) -> EventPane {
    EventPane {
        pane_id: id.into(),
        tab_id: "tab".into(),
        workspace_id: "workspace".into(),
        terminal_id: Some(
            if id.starts_with("card") {
                "card-terminal"
            } else {
                "owner-terminal"
            }
            .into(),
        ),
    }
}
fn routes() -> Routes {
    Routes {
        owner: pane("owner"),
        card: pane("card"),
    }
}
fn moved(previous: &str, next: &str) -> serde_json::Value {
    json!({"event":"pane_moved", "data":{"type":"pane_moved", "previous_pane_id":previous,
        "previous_tab_id":"tab", "previous_workspace_id":"workspace",
        "pane":{"pane_id":next,"tab_id":"tab","workspace_id":"workspace","terminal_id":if previous.starts_with("card") {"card-terminal"} else {"owner-terminal"}}}})
}
#[test]
fn early_id_filtering_and_drained_native_bursts_signal_one_read_then_owner_close_without_requests()
{
    let (_directory, server, client, events, ui) = fixture();
    let mut native = Native::subscribe(&client, routes(), events.clone()).unwrap();
    native.activate(Bootstrap::Initial, routes());
    for _ in 0..32 {
        // A deliberately incomplete unrelated payload must be filtered before full decode.
        server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"unrelated"}}}));
    }
    server.emit(moved("owner", "next"));
    server.emit(moved("next", "last"));
    server.emit(moved("card", "card-next"));
    server.emit(json!({"event":"pane_closed", "data":{"type":"pane_closed", "pane_id":"last","workspace_id":"workspace"}}));
    let mut reads = 0;
    loop {
        match ui.recv_timeout(Duration::from_secs(3)).unwrap() {
            PaneEvent::Changed => reads += 1,
            PaneEvent::Exit => break,
            PaneEvent::Failed(error) => panic!("{error}"),
        }
    }
    assert_eq!(reads, 1, "a queued burst needs one display read");
    assert_eq!(
        events.drain().len(),
        3,
        "filtered events must not reach the reader"
    );
    assert_eq!(native.routes.lock().unwrap().routes().owner.pane_id, "last");
    assert_eq!(
        native.routes.lock().unwrap().routes().card.pane_id,
        "card-next"
    );
    native.stop();
    assert_eq!(server.methods(), ["ping", "events.subscribe"]);
}
#[test]
fn native_recovery_gets_one_full_read_and_new_ids_before_receiving_again() {
    let (_directory, server, client, events, ui) = fixture();
    let mut native = Native::subscribe(&client, routes(), events.clone()).unwrap();
    native.activate(Bootstrap::Initial, routes());
    server.clear();
    server.disconnect_subscriptions();
    assert!(matches!(
        ui.recv_timeout(Duration::from_secs(3)).unwrap(),
        PaneEvent::Changed
    ));
    assert!(matches!(
        events.drain().as_slice(),
        [Change::NativeRecovered]
    ));
    assert_eq!(server.methods(), ["events.subscribe"]);
    client.session_snapshot().unwrap();
    let mut recovered = routes();
    recovered.owner = pane("during-gap");
    native.activate(Bootstrap::Recovery, recovered);
    server.emit(moved("during-gap", "after-gap"));
    assert!(matches!(
        ui.recv_timeout(Duration::from_secs(3)).unwrap(),
        PaneEvent::Changed
    ));
    assert!(matches!(events.drain().as_slice(), [Change::Native]));
    assert_eq!(
        native.routes.lock().unwrap().routes().owner.pane_id,
        "after-gap"
    );
    server.disconnect_subscriptions();
    assert!(matches!(
        ui.recv_timeout(Duration::from_secs(3)).unwrap(),
        PaneEvent::Failed(_)
    ));
    native.stop();
    assert_eq!(server.methods(), ["events.subscribe", "session.snapshot"]);
}
#[test]
fn shutdown_rejects_a_recovery_queued_after_the_pending_batch_was_drained() {
    let (_directory, server, _client, events, ui) = fixture();
    events.stop();
    events.push(Change::NativeRecovered);
    assert!(events.drain().is_empty());
    assert!(ui.try_recv().is_err());
    assert_eq!(server.methods(), ["ping"]);
}
#[test]
fn stopping_during_buffered_recovery_needs_no_full_read() {
    let (_directory, server, client, events, ui) = fixture();
    let mut native = Native::subscribe(&client, routes(), events).unwrap();
    native.activate(Bootstrap::Initial, routes());
    server.clear();
    server.disconnect_subscriptions();
    assert!(matches!(
        ui.recv_timeout(Duration::from_secs(3)).unwrap(),
        PaneEvent::Changed
    ));
    native.stop();
    assert_eq!(server.methods(), ["events.subscribe"]);
}
