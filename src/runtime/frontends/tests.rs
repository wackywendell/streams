use super::*;
use crate::fake_herdr::FakeHerdr;

fn caller(environment: BTreeMap<String, String>) -> Caller {
    Caller {
        scope: WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        },
        environment,
        cwd: "/tmp".into(),
        timeout: 1.0,
    }
}

#[test]
fn hc_without_a_terminal_associates_and_notifies_nothing_and_never_connects() {
    // No HC_SOCKET or HC_SESSION either: any connection attempt would fail.
    let mut frontend = connect(FrontendId::Hc, caller(BTreeMap::new()));
    frontend.associate_caller("stream").unwrap();
    frontend.notify_saved().unwrap();
    assert_eq!(frontend.context_stream(), None);
    assert!(frontend.caller_stream().is_err());
}

#[test]
fn hc_with_a_terminal_but_no_endpoint_reports_the_failure() {
    let environment = BTreeMap::from([("HC_TERMINAL_ID".into(), "t1".into())]);
    let mut frontend = connect(FrontendId::Hc, caller(environment));
    assert!(frontend.associate_caller("stream").is_err());
    assert!(frontend.notify_saved().is_err());
    assert_eq!(frontend.context_stream(), None);
}

#[test]
fn observed_herdr_answers_from_the_child_observation_without_a_snapshot() {
    let temp = tempfile::Builder::new()
        .prefix("frontends-")
        .tempdir_in("/tmp")
        .unwrap();
    let server = FakeHerdr::start(&temp.path().join("herdr.sock"), |method, _| {
        panic!("unexpected {method}")
    });
    let environment = BTreeMap::from([
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "owner".into()),
        (
            "HERDR_SOCKET_PATH".into(),
            server.socket.to_str().unwrap().into(),
        ),
    ]);
    let client = HerdrClient::connect(environment.clone(), 1.0).unwrap();
    for observed in [Some("stream".to_string()), None] {
        let mut frontend =
            HerdrFrontend::observed(caller(environment.clone()), &client, observed.clone())
                .unwrap();
        assert_eq!(frontend.context_stream(), observed);
        assert_eq!(frontend.caller_stream().unwrap(), observed);
        assert_eq!(frontend.workspace_stream().unwrap(), observed);
        frontend.associate_caller("stream").unwrap();
        frontend.notify_saved().unwrap();
    }
    assert_eq!(server.methods(), ["ping"]);
}

#[test]
fn cmux_names_no_stream_and_sees_nothing_running() {
    let mut frontend = connect(FrontendId::Cmux, caller(BTreeMap::new()));
    assert_eq!(frontend.context_stream(), None);
    assert!(frontend.workspace_stream().is_err());
    let session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "stream",
        "pi",
        "native",
        "Work",
    )
    .unwrap();
    let selected = SelectedConversation {
        session: &session,
        locator: None,
    };
    assert!(!frontend.running(&selected).unwrap());
}

#[test]
fn an_hc_opening_always_reports_the_hc_backend() {
    assert_eq!(serde_json::to_value(HcBackend).unwrap(), "hc");
}
