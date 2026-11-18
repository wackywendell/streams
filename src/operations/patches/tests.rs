use super::*;
use crate::model::{Session, Stream};
use serde_json::{Value, json};

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
        assert_eq!(access.permits(Actor::User, Channel::Agent), user_agent);
        assert_eq!(access.permits(Actor::User, Channel::Cli), user_cli);
        assert_eq!(access.permits(Actor::Automatic, Channel::Agent), automatic);
        assert_eq!(access.permits(Actor::Automatic, Channel::Cli), automatic);
        assert!(!access.permits(Actor::Native, Channel::Cli));
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
    let schemas = PatchSchemas::new(Some(&stream()), Some(&session()), TitleDefaults::default());
    assert_eq!(serde_json::to_value(&schemas).unwrap(), fixture);
    let mut manual = stream();
    manual.goal_mode = NamingMode::Manual;
    manual.title_mode = TitleMode::Manual;
    let schemas = serde_json::to_value(PatchSchemas::new(
        Some(&manual),
        None,
        TitleDefaults::default(),
    ))
    .unwrap();
    assert_eq!(schemas["stream"]["automatic"], json!([]));
    assert_eq!(schemas["stream"]["user"], fixture["stream"]["user"]);
    assert!(schemas.get("session").is_none());
}

#[test]
fn short_forms_are_cut_to_one_short_line() {
    let long = "Ünïcödé  short\nform that runs well past the limit";
    let patch: SessionPatch = serde_json::from_value(json!({"recap_short": long})).unwrap();
    let short = patch.normalized(&session(), Actor::Automatic).recap_short;
    assert_eq!(short, Some(Some("Ünïcödé short form that".into())));
    assert_eq!(
        crate::model::short_text("  exact  "),
        "exact",
        "a short form is trimmed"
    );
    let patch: StreamPatch = serde_json::from_value(json!({"goal_short": long})).unwrap();
    let short = patch.normalized(&stream(), Actor::User).goal_short;
    assert_eq!(short.flatten().map(|text| text.chars().count()), Some(23));
}

#[test]
fn automatic_short_goal_without_a_goal_is_dropped() {
    let mut goalless = stream();
    goalless.goal = None;
    let patch: StreamPatch = serde_json::from_value(json!({"goal_short": "Short"})).unwrap();
    assert_eq!(
        patch
            .clone()
            .normalized(&goalless, Actor::Automatic)
            .goal_short,
        Some(None)
    );
    assert_eq!(
        patch.normalized(&goalless, Actor::User).goal_short,
        Some(Some("Short".into())),
        "a user's short goal reaches validation, which refuses it"
    );
    let patch: StreamPatch =
        serde_json::from_value(json!({"goal": null, "goal_short": "Short"})).unwrap();
    assert_eq!(
        patch.normalized(&stream(), Actor::Automatic).goal_short,
        Some(None)
    );
}

#[test]
fn title_modes_may_inherit_but_goal_modes_may_not() {
    assert!(serde_json::from_value::<SessionPatch>(json!({"title_mode": "inherit"})).is_ok());
    assert!(serde_json::from_value::<StreamPatch>(json!({"title_mode": "inherit"})).is_ok());
    assert!(serde_json::from_value::<StreamPatch>(json!({"goal_mode": "inherit"})).is_err());
    assert!(serde_json::from_value::<SessionPatch>(json!({"title_mode": "auto"})).is_err());
}
