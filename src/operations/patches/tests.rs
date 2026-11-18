use super::*;
use crate::model::{Session, Stream};

fn stream() -> Stream {
    Stream::new("stream", "Title", Some("Goal".into()), "host").unwrap()
}
fn session() -> Session {
    Session::new("session", "host", "stream", "pi", "native", "Title").unwrap()
}

#[test]
fn each_field_names_who_may_set_it() {
    use Access::*;
    let stream: Vec<_> = StreamField::ALL
        .iter()
        .map(|field| (field.name(), field.access()))
        .collect();
    assert_eq!(
        stream,
        [
            ("title", Report),
            ("title_mode", User),
            ("goal", Report),
            ("goal_short", Report),
            ("goal_mode", User),
            ("status", User),
            ("repositories", Cli),
        ]
    );
    let session: Vec<_> = SessionField::ALL
        .iter()
        .map(|field| (field.name(), field.access()))
        .collect();
    assert_eq!(
        session,
        [
            ("title", Report),
            ("title_mode", User),
            ("status", Report),
            ("recap", Report),
            ("recap_short", Report),
            ("state", Report),
            ("state_short", Report),
            ("remaining", Report),
            ("remaining_short", Report),
        ]
    );
    for (access, user_agent, user_cli, automatic) in [
        (Report, true, true, true),
        (User, true, true, false),
        (Cli, false, true, false),
    ] {
        assert_eq!(access.permits(Actor::User, true), user_agent);
        assert_eq!(access.permits(Actor::User, false), user_cli);
        assert_eq!(access.permits(Actor::Automatic, true), automatic);
        assert_eq!(access.permits(Actor::Automatic, false), automatic);
        assert!(!access.permits(Actor::Native, false));
    }
}

#[test]
fn absent_fields_are_unchanged_and_null_clears_only_optional_fields() {
    let patch: StreamPatch = serde_json::from_value(json!({"goal": null})).unwrap();
    assert_eq!(patch.goal, Some(None));
    assert_eq!(patch.fields(), [StreamField::Goal]);
    let patch: StreamPatch = serde_json::from_value(json!({})).unwrap();
    assert!(patch.is_empty());
    for value in [
        json!({"title": null}),
        json!({"status": null}),
        json!({"owner_host_id": "other"}),
        json!({"references": []}),
        json!({"status": "unknown"}),
    ] {
        assert!(
            serde_json::from_value::<StreamPatch>(value.clone()).is_err(),
            "{value}"
        );
    }
    for value in [
        json!({"recap": null}),
        json!({"stream_id": "other"}),
        json!({"checkout": null}),
    ] {
        assert!(
            serde_json::from_value::<SessionPatch>(value.clone()).is_err(),
            "{value}"
        );
    }
    let patch: SessionPatch =
        serde_json::from_value(json!({"recap": "", "recap_short": null})).unwrap();
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({"recap": "", "recap_short": null})
    );
}

#[test]
fn between_and_apply_round_trip() {
    let before = stream();
    let mut after = before.clone();
    after.goal = None;
    after.status = Status::Done;
    let patch = StreamPatch::between(&before, &after);
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({"goal": null, "status": "done"})
    );
    let mut applied = before.clone();
    patch.apply(&mut applied);
    assert_eq!(applied, after);
}

#[test]
fn schemas_match_the_shared_fixture_and_omit_protected_text() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/agent-patch-schemas.json"
    ))
    .unwrap();
    assert_eq!(
        agent_patch_schemas(Some(&stream()), Some(&session()), TitleDefaults::default()),
        fixture
    );
    let mut manual = stream();
    manual.goal_mode = NamingMode::Manual;
    manual.title_mode = TitleMode::Manual;
    let schemas = agent_patch_schemas(Some(&manual), None, TitleDefaults::default());
    assert_eq!(schemas["stream"]["automatic"]["properties"], json!({}));
    assert_eq!(
        schemas["stream"]["user"]["properties"],
        fixture["stream"]["user"]["properties"]
    );
    assert!(schemas.get("session").is_none());
}
