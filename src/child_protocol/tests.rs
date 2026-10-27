use super::*;
use serde_json::json;

#[test]
fn shared_wire_fixtures_round_trip_without_cli_translation() {
    let fixtures = codec::load_json(include_str!("../../tests/fixtures/pi-child-v2.json")).unwrap();
    for value in fixtures["client"].as_array().unwrap() {
        let frame = decode_client(&value.to_string()).unwrap();
        assert_eq!(serde_json::to_value(frame).unwrap(), *value);
    }
    for value in fixtures["server"].as_array().unwrap() {
        let frame = decode_server(&value.to_string()).unwrap();
        assert_eq!(serde_json::to_value(frame).unwrap(), *value);
    }
}

#[test]
fn title_defaults_have_closed_targets_and_resolved_modes_without_native_proofs() {
    for target in ["stream", "session"] {
        for mode in ["automatic", "manual"] {
            let request = json!({"kind":"set_title_default","target":target,"mode":mode});
            let frame = json!({"type":"request","id":1,"request":request});
            assert_eq!(
                serde_json::to_value(decode_client(&frame.to_string()).unwrap()).unwrap(),
                frame
            );
        }
    }
    for (field, invalid) in [
        ("target", json!("host")),
        ("target", json!("Stream")),
        ("target", json!(null)),
        ("target", json!({"stream":null})),
        ("mode", json!("inherit")),
        ("mode", json!("Automatic")),
        ("mode", json!(null)),
        ("mode", json!(false)),
        ("mode", json!({"automatic":null})),
        ("native", json!({})),
        ("catalog_root", json!("/other")),
        ("argv", json!(["config", "set"])),
    ] {
        let mut request = json!({"kind":"set_title_default","target":"session","mode":"manual"});
        request[field] = invalid;
        assert!(
            decode_client(&json!({"type":"request","id":1,"request":request}).to_string()).is_err()
        );
    }
    for missing in ["target", "mode"] {
        let mut request = json!({"kind":"set_title_default","target":"session","mode":"manual"});
        request.as_object_mut().unwrap().remove(missing);
        assert!(
            decode_client(&json!({"type":"request","id":1,"request":request}).to_string()).is_err()
        );
    }
}

#[test]
fn request_ids_are_positive_javascript_safe_integers() {
    for id in [1, 9_007_199_254_740_991] {
        assert_eq!(RequestId::new(id).unwrap().get(), id);
        let frame = json!({"type":"cancel","id":id});
        assert!(decode_client(&frame.to_string()).is_ok());
    }
    for id in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(9_007_199_254_740_992u64),
        json!("1"),
        Value::Null,
    ] {
        let frame = json!({"type":"cancel","id":id});
        assert!(decode_client(&frame.to_string()).is_err(), "{frame}");
    }
}

#[test]
fn native_intents_cannot_carry_an_identity_location_or_expectation() {
    for kind in [
        "session_started",
        "attach",
        "open_card",
        "close_card",
        "context",
    ] {
        for field in [
            "session_id",
            "native_session_id",
            "pane_id",
            "terminal_id",
            "expect",
            "native",
            "argv",
        ] {
            let mut request = json!({"kind":kind});
            request[field] = json!("not-authority");
            let frame = json!({"type":"request","id":1,"request":request});
            assert!(decode_client(&frame.to_string()).is_err(), "{frame}");
        }
    }
}

#[test]
fn unsupported_frames_fields_and_positional_structs_fail() {
    for frame in [
        json!([]),
        json!(null),
        json!({"type":"request","id":1,"argv":["native","open"]}),
        json!({"type":"request","id":1,"request":{"kind":"open_card"},"input":null}),
        json!({"type":"request","id":1,"request":{"kind":"project"}}),
        json!({"type":"request","id":1,"request":{"kind":"context","stream_id":5}}),
        json!({"type":"request","id":1,"request":{"kind":"apply","request":[]}}),
        json!({"type":"request","id":1,"request":{"kind":"enroll","request":null}}),
        json!({"type":"request","id":1,"request":{"kind":"capture_inputs","target":{"kind":"inventory","workspace_id":"w"}}}),
        json!({"type":"request","id":1,"request":{"kind":"preview","catalog":{},"action":{"apply":null},"request":{}}}),
        json!({"type":"request","id":1,"request":{"kind":"sync","event":"start"}}),
        json!({"type":"cancel","id":1,"request":{"kind":"open_card"}}),
    ] {
        assert!(decode_client(&frame.to_string()).is_err(), "{frame}");
    }
    for frame in [
        json!({"type":"ready","protocol":1}),
        json!({"type":"ready","protocol":"2"}),
        json!({"type":"ready","protocol":2,"scope":null}),
        json!({"type":"scope","scope":{"pane_id":"w:p1"}}),
        json!({"type":"view","view":["Name"]}),
        json!({"type":"view","view":{}}),
        json!({"type":"view","view":{"desired_pi_name":"Name","pane_id":"w:p1"}}),
        json!({"type":"response","id":1,"outcome":{"status":"error","error":["runtime","refused"]}}),
        json!({"type":"response","id":1,"outcome":{"status":"success","result":null,"warnings":[]}}),
        json!({"type":"response","id":1,"outcome":{"status":"success","result":[]}}),
        json!({"type":"response","id":1,"outcome":{"status":"success","result":null}}),
        json!({"type":"response","id":1,"outcome":{"status":"success","result":true}}),
        json!({"type":"response","id":1,"outcome":{"status":"saved_with_warnings","result":{},"warnings":[]}}),
    ] {
        assert!(decode_server(&frame.to_string()).is_err(), "{frame}");
    }
}

#[test]
fn duplicate_fields_and_trailing_json_fail_at_every_nesting_level() {
    for line in [
        r#"{"type":"cancel","id":1,"id":2}"#,
        r#"{"type":"request","id":1,"request":{"kind":"open_card","kind":"close_card"}}"#,
        r#"{"type":"request","id":1,"request":{"kind":"apply","request":{"actor":"user","actor":"automatic"}}}"#,
        r#"{"type":"cancel","id":1} {}"#,
    ] {
        assert!(decode_client(line).is_err(), "{line}");
    }
    assert!(
        decode_server(r#"{"type":"view","view":{"desired_pi_name":"A","desired_pi_name":"B"}}"#)
            .is_err()
    );
}

#[test]
fn catalog_packets_remain_domain_inputs_not_native_proofs() {
    let packet =
        json!({"version":2,"catalog_source":"source","captured":{"membership":"original"}});
    let frame = json!({"type":"request","id":1,"request":{"kind":"enroll","request":packet}});
    let ClientFrame::Request {
        request: Request::Enroll { request },
        ..
    } = decode_client(&frame.to_string()).unwrap()
    else {
        panic!("enrollment intent expected");
    };
    assert_eq!(Value::Object(request), packet);
    assert_eq!(
        decode_client(r#"{"type":"request","id":1,"request":{"kind":"context","stream_id":null}}"#)
            .unwrap(),
        decode_client(r#"{"type":"request","id":1,"request":{"kind":"context"}}"#).unwrap(),
    );
}

#[test]
fn accepted_save_warnings_and_background_facts_are_distinct_from_failure() {
    let result =
        json!({"operation_id":"accepted","completion_error":{"message":"projection failed"}});
    let frame = ServerFrame::Response {
        id: RequestId::new(1).unwrap(),
        outcome: Outcome::SavedWithWarnings {
            result: result.as_object().unwrap().clone(),
            warnings: Warnings::new(vec!["Projection failed; title remains saved".into()]).unwrap(),
        },
    };
    let decoded = decode_server(&serde_json::to_string(&frame).unwrap()).unwrap();
    assert_eq!(decoded, frame);
    let ServerFrame::Response {
        outcome: Outcome::SavedWithWarnings {
            result: retained, ..
        },
        ..
    } = decoded
    else {
        panic!("save must not become a failure");
    };
    assert_eq!(Value::Object(retained), result);
    assert!(Warnings::new(vec![]).is_err());
    assert_eq!(
        Warnings::new(vec!["Retained".into()]).unwrap().messages(),
        &["Retained"]
    );
    assert_eq!(
        serde_json::to_value(ServerFrame::View {
            view: UiFacts {
                desired_pi_name: None
            }
        })
        .unwrap(),
        json!({"type":"view","view":{"desired_pi_name":null}}),
    );
}
