use super::*;
use crate::model::Checkout;

fn proposal(r#ref: Option<&str>) -> AutomaticReferenceValue {
    AutomaticReferenceValue {
        name: "Design".into(),
        purpose: "Guide the work".into(),
        role: ReferenceRole::Brief,
        state: ReferenceState::Active,
        target: AutomaticReferenceTarget::Document(AutomaticDocumentTarget {
            repository_url: "https://example.com/repo".into(),
            path: "docs/design.md".into(),
            r#ref: r#ref.map(Into::into),
        }),
    }
}
fn session(repository_url: &str) -> Session {
    let mut session = Session::new("session", "host", "stream", "pi", "native", "Title").unwrap();
    session.checkout = Some(Checkout {
        repository: "repo".into(),
        repository_url: Some(repository_url.into()),
        r#ref: "topic".into(),
        directory_within_checkout: ".".into(),
    });
    session
}
fn document(value: &ReferenceValue) -> &RepositoryDocument {
    match &value.target {
        ReferenceTarget::Document(document) => document,
        _ => panic!("document target"),
    }
}

#[test]
fn materialized_hints_come_from_the_saved_row_or_a_matching_checkout() {
    let first = proposal(None).materialize(None, &session("https://example.com/repo"));
    let hints = document(&first);
    assert_eq!(hints.r#ref.as_deref(), Some("topic"));
    assert_eq!(hints.source_session_id.as_deref(), Some("session"));
    assert_eq!(hints.commit, None);
    let other = proposal(None).materialize(None, &session("https://example.com/other"));
    assert_eq!(document(&other).r#ref, None);

    let mut saved = first.clone();
    if let ReferenceTarget::Document(document) = &mut saved.target {
        document.r#ref = Some("saved".into());
        document.commit = Some("commit".into());
        document.source_session_id = Some("origin".into());
    }
    let saved = Reference::new(saved).unwrap();
    let kept = proposal(None).materialize(Some(&saved), &session("https://example.com/repo"));
    assert_eq!(kept.target, saved.target);
    let replaced = proposal(Some("replacement"))
        .materialize(Some(&saved), &session("https://example.com/repo"));
    assert_eq!(document(&replaced).r#ref.as_deref(), Some("replacement"));
    assert_eq!(document(&replaced).commit.as_deref(), Some("commit"));
}
