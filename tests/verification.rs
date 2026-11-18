//! Bounded Temper formal verification of the report-admission model, and conformance of the
//! production `ReportAdmission` predicate and concrete edits with it.

use std::collections::{BTreeSet, VecDeque};
use std::sync::OnceLock;

use serde_json::{Map, Value, json};
use streams::model::{
    NamingMode, Record, Reference, ReferenceRole, ReferenceState, ReferenceTarget, ReferenceValue,
    Session, Status, Stream, TitleDefaults, TitleMode,
};
use streams::operations::edits::{
    ReportAdmission, SessionEdit, StreamEdit, report_admission_context,
};
use streams::operations::membership::{MoveIntent, apply_move};
use streams::operations::{Actor, DomainOutcome, Reconciliation};
use temper_jit::table::{Effect, EvalContext, TransitionTable, effect_args};
use temper_verify::cascade::{CascadeLevel, VerificationCascade};

const REPORT_ADMISSION_IOA: &str = include_str!("../specs/report_admission.ioa.toml");

fn report_admission_table() -> &'static TransitionTable {
    static TABLE: OnceLock<TransitionTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        TransitionTable::try_from_ioa_source(REPORT_ADMISSION_IOA)
            .expect("valid report-admission IOA specification")
    })
}

fn report_admission_allows(action: &str, ctx: &EvalContext) -> bool {
    let table = report_admission_table();
    table
        .evaluate_ctx(&table.initial_state, ctx, action)
        .is_some_and(|result| result.success)
}

fn eval_context(admission: ReportAdmission) -> EvalContext {
    let mut ctx = EvalContext::default();
    for (name, value) in [
        ("in_captured_stream", admission.in_captured_stream),
        ("membership_unchanged", admission.membership_unchanged),
        ("session_inputs_clean", admission.session_inputs_clean),
        ("stream_inputs_clean", admission.stream_inputs_clean),
        ("session_protected", admission.session_protected),
        ("stream_protected", admission.stream_protected),
    ] {
        ctx.booleans.insert(name.into(), value);
    }
    ctx
}

#[test]
fn production_admission_predicate_matches_ioa_guards_for_every_combination() {
    for bits in 0u8..64 {
        let admission = ReportAdmission {
            in_captured_stream: bits & 1 != 0,
            membership_unchanged: bits & 2 != 0,
            session_inputs_clean: bits & 4 != 0,
            stream_inputs_clean: bits & 8 != 0,
            session_protected: bits & 16 != 0,
            stream_protected: bits & 32 != 0,
        };
        let ctx = eval_context(admission);
        assert_eq!(
            admission.admits_session(),
            report_admission_allows("AdmitSessionReport", &ctx),
            "{admission:?}"
        );
        assert_eq!(
            admission.admits_stream(),
            report_admission_allows("AdmitStreamReport", &ctx),
            "{admission:?}"
        );
    }
    assert_eq!(
        eval_context(ReportAdmission::default()).booleans,
        report_admission_table().initial_values.booleans
    );
}

fn changes(value: Value) -> Map<String, Value> {
    value.as_object().expect("json object").clone()
}

#[derive(Clone, Copy, Debug)]
enum StreamProtectionKind {
    TitleModeManual,
    GoalModeManual,
    InheritedDefaultFlip,
}

#[derive(Clone, Debug)]
struct ConcreteWorld {
    protection_kind: StreamProtectionKind,
    session: Session,
    stream_a: Stream,
    stream_b: Stream,
    current_defaults: TitleDefaults,
    session_report: SessionEdit,
    stream_report: StreamEdit,
}

impl ConcreteWorld {
    fn new(protection_kind: StreamProtectionKind) -> Self {
        let mut session = Session::new(
            Session::id_for("host", "pi", "native"),
            "host",
            "a",
            "pi",
            "native",
            "Initial work",
        )
        .unwrap();
        let mut stream_a =
            Stream::new("a", "Stream A", Some("Initial goal".into()), "host").unwrap();
        let stream_b = Stream::new("b", "Stream B", Some("Other goal".into()), "host").unwrap();
        let initial_title_mode = match protection_kind {
            StreamProtectionKind::InheritedDefaultFlip => TitleMode::Inherit,
            StreamProtectionKind::TitleModeManual | StreamProtectionKind::GoalModeManual => {
                TitleMode::Automatic
            }
        };
        session.title_mode = initial_title_mode;
        stream_a.title_mode = initial_title_mode;

        let session_report = SessionEdit::new(
            "report-session",
            &session,
            &changes(json!({
                "title": "Generated session title",
                "state": "Generated session state",
                "remaining": "Generated remaining work"
            })),
            Actor::Automatic,
            NamingMode::Automatic,
        )
        .unwrap();
        let stream_report = StreamEdit::new(
            "report-stream",
            &stream_a,
            &changes(json!({
                "title": "Generated stream title",
                "goal": "Generated stream goal",
                "goal_short": "Generated goal"
            })),
            Actor::Automatic,
            Some(session.clone()),
            NamingMode::Automatic,
        )
        .unwrap();

        Self {
            protection_kind,
            session,
            stream_a,
            stream_b,
            current_defaults: TitleDefaults::default(),
            session_report,
            stream_report,
        }
    }

    fn current_stream(&self) -> &Stream {
        if self.session.stream_id == self.stream_a.id {
            &self.stream_a
        } else {
            &self.stream_b
        }
    }

    fn project(&self) -> EvalContext {
        eval_context(report_admission_context(
            &self.session_report,
            &self.stream_report,
            &self.session,
            &self.stream_a,
            self.current_defaults,
        ))
    }

    fn apply_env_action(&mut self, action: &str) {
        match action {
            "MoveToOtherStream" => {
                let intent = MoveIntent::new("host", "pi", "native", "b").unwrap();
                self.session = apply_move(&self.session, &self.stream_a, &self.stream_b, &intent)
                    .unwrap()
                    .expect("move A -> B changes membership");
            }
            "ReturnToCapturedStream" => {
                let intent = MoveIntent::new("host", "pi", "native", "a").unwrap();
                self.session = apply_move(&self.session, &self.stream_b, &self.stream_a, &intent)
                    .unwrap()
                    .expect("move B -> A changes membership");
            }
            "DirtySessionInputs" => {
                let edit = SessionEdit::new(
                    "user-session-input",
                    &self.session,
                    &changes(json!({"remaining": "User updated remaining"})),
                    Actor::User,
                    self.current_defaults.session,
                )
                .unwrap();
                if let Reconciliation::Changes { record, .. } = edit.apply(
                    &self.session,
                    self.current_stream(),
                    self.current_defaults.session,
                ) {
                    let Record::Session(updated) = *record else {
                        unreachable!()
                    };
                    self.session = updated;
                }
            }
            "DirtyStreamInputs" => {
                let edit = StreamEdit::new(
                    "user-stream-input",
                    &self.stream_a,
                    &changes(json!({
                        "goal": "User updated goal",
                        "goal_mode": self.stream_a.goal_mode
                    })),
                    Actor::User,
                    None,
                    self.current_defaults.stream,
                )
                .unwrap();
                if let Reconciliation::Changes { record, .. } =
                    edit.apply(&self.stream_a, None, self.current_defaults.stream)
                {
                    let Record::Stream(updated) = *record else {
                        unreachable!()
                    };
                    self.stream_a = updated;
                }
            }
            "ProtectSessionText" => match self.protection_kind {
                StreamProtectionKind::TitleModeManual | StreamProtectionKind::GoalModeManual => {
                    let edit = SessionEdit::new(
                        "protect-session-title",
                        &self.session,
                        &changes(json!({"title_mode": "manual"})),
                        Actor::User,
                        self.current_defaults.session,
                    )
                    .unwrap();
                    if let Reconciliation::Changes { record, .. } = edit.apply(
                        &self.session,
                        self.current_stream(),
                        self.current_defaults.session,
                    ) {
                        let Record::Session(updated) = *record else {
                            unreachable!()
                        };
                        self.session = updated;
                    }
                }
                StreamProtectionKind::InheritedDefaultFlip => {
                    self.current_defaults.session = NamingMode::Manual;
                }
            },
            "ProtectStreamText" => match self.protection_kind {
                StreamProtectionKind::TitleModeManual | StreamProtectionKind::GoalModeManual => {
                    let patch = match self.protection_kind {
                        StreamProtectionKind::TitleModeManual => json!({"title_mode": "manual"}),
                        StreamProtectionKind::GoalModeManual => json!({"goal_mode": "manual"}),
                        StreamProtectionKind::InheritedDefaultFlip => unreachable!(),
                    };
                    let edit = StreamEdit::new(
                        "protect-stream-text",
                        &self.stream_a,
                        &changes(patch),
                        Actor::User,
                        None,
                        self.current_defaults.stream,
                    )
                    .unwrap();
                    if let Reconciliation::Changes { record, .. } =
                        edit.apply(&self.stream_a, None, self.current_defaults.stream)
                    {
                        let Record::Stream(updated) = *record else {
                            unreachable!()
                        };
                        self.stream_a = updated;
                    }
                }
                StreamProtectionKind::InheritedDefaultFlip => {
                    self.current_defaults.stream = NamingMode::Manual;
                }
            },
            other => panic!("unexpected environment action: {other}"),
        }
    }
}

fn step_ioa(table: &TransitionTable, ctx: &EvalContext, action: &str) -> Option<EvalContext> {
    let result = table.evaluate_ctx(&table.initial_state, ctx, action)?;
    if !result.success {
        return None;
    }
    let mut next = ctx.clone();
    for effect in result.effects {
        match effect {
            Effect::SetState(_) => {}
            Effect::SetBool { var, value } => {
                let resolved =
                    effect_args::boolean(&value, &next.booleans, &Value::Null).expect("bool expr");
                *next.booleans.get_mut(&var).expect("declared bool") = resolved;
            }
            other => panic!("unexpected effect in ReportAdmission: {other:?}"),
        }
    }
    Some(next)
}

#[test]
fn report_admission_passes_all_four_temper_levels() {
    let report = VerificationCascade::from_ioa(REPORT_ADMISSION_IOA)
        .with_sim_seeds(5)
        .with_prop_test_cases(100)
        .run();
    assert_eq!(report.levels.len(), 4);
    for expected in [
        CascadeLevel::SymbolicVerification,
        CascadeLevel::ModelCheck,
        CascadeLevel::Simulation,
        CascadeLevel::PropertyTest,
    ] {
        assert!(
            report
                .level_result(expected)
                .is_some_and(|level| level.passed),
            "level {expected:?} failed: {report:#?}"
        );
    }
    assert!(report.all_passed, "{report:#?}");
}

#[test]
fn concrete_operations_commute_with_ioa_and_agree_on_admission_across_reachable_states() {
    let table = report_admission_table();
    let env_actions = [
        "MoveToOtherStream",
        "ReturnToCapturedStream",
        "DirtySessionInputs",
        "DirtyStreamInputs",
        "ProtectSessionText",
        "ProtectStreamText",
    ];

    for kind in [
        StreamProtectionKind::TitleModeManual,
        StreamProtectionKind::GoalModeManual,
        StreamProtectionKind::InheritedDefaultFlip,
    ] {
        let initial_world = ConcreteWorld::new(kind);
        let initial_ctx = EvalContext {
            booleans: table.initial_values.booleans.clone(),
            ..EvalContext::default()
        };
        assert_eq!(
            initial_world.project().booleans,
            initial_ctx.booleans,
            "initial concrete projection must match IOA initial state ({kind:?})"
        );

        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        visited.insert(initial_ctx.booleans.clone());
        queue.push_back((initial_ctx, initial_world, Vec::<&'static str>::new()));

        while let Some((ioa_ctx, world, trace)) = queue.pop_front() {
            let projected = world.project();
            assert_eq!(
                projected.booleans, ioa_ctx.booleans,
                "concrete projection diverged from IOA state after {trace:?} ({kind:?})"
            );

            // Check SessionEdit admission equivalence.
            let ioa_admits_session = report_admission_allows("AdmitSessionReport", &ioa_ctx);
            let session_reconciliation = world.session_report.apply(
                &world.session,
                world.current_stream(),
                world.current_defaults.session,
            );
            assert_eq!(
                ioa_admits_session,
                matches!(session_reconciliation, Reconciliation::Changes { .. }),
                "SessionEdit admission disagreed with IOA after {trace:?} ({kind:?}): {session_reconciliation:?}"
            );
            if let Reconciliation::Changes { record, .. } = session_reconciliation {
                let Record::Session(applied) = *record else {
                    unreachable!()
                };
                assert_eq!(applied.stream_id, world.session.stream_id);
                assert_eq!(
                    applied.previous_stream_ids,
                    world.session.previous_stream_ids
                );
                assert_eq!(
                    world
                        .session_report
                        .apply(
                            &applied,
                            world.current_stream(),
                            world.current_defaults.session
                        )
                        .outcome(),
                    DomainOutcome::NoChange
                );
            } else {
                assert_eq!(
                    session_reconciliation.outcome(),
                    DomainOutcome::Superseded,
                    "rejected automatic SessionEdit must be Superseded after {trace:?}"
                );
            }

            // Check StreamEdit admission equivalence on the captured Stream.
            let ioa_admits_stream = report_admission_allows("AdmitStreamReport", &ioa_ctx);
            let stream_reconciliation = world.stream_report.apply(
                &world.stream_a,
                Some(&world.session),
                world.current_defaults.stream,
            );
            assert_eq!(
                ioa_admits_stream,
                matches!(stream_reconciliation, Reconciliation::Changes { .. }),
                "StreamEdit admission disagreed with IOA after {trace:?} ({target:?}): {stream_reconciliation:?}",
                target = kind
            );
            if let Reconciliation::Changes { record, .. } = stream_reconciliation {
                let Record::Stream(applied) = *record else {
                    unreachable!()
                };
                assert!(
                    ioa_ctx.booleans["in_captured_stream"]
                        && ioa_ctx.booleans["membership_unchanged"]
                );
                assert_eq!(
                    world
                        .stream_report
                        .apply(
                            &applied,
                            Some(&world.session),
                            world.current_defaults.stream
                        )
                        .outcome(),
                    DomainOutcome::NoChange
                );
            } else {
                assert_eq!(
                    stream_reconciliation.outcome(),
                    DomainOutcome::Superseded,
                    "rejected automatic StreamEdit must be Superseded after {trace:?}"
                );
            }

            // Attempting to apply a captured Stream A report to Stream B is always a Conflict.
            let cross_target = world.stream_report.apply(
                &world.stream_b,
                Some(&world.session),
                world.current_defaults.stream,
            );
            assert_eq!(cross_target.outcome(), DomainOutcome::Conflict);
            assert_eq!(cross_target.conflicts(), &["identity"]);

            for action in env_actions {
                if let Some(next_ioa) = step_ioa(table, &ioa_ctx, action) {
                    let mut next_world = world.clone();
                    next_world.apply_env_action(action);
                    let mut next_trace = trace.clone();
                    next_trace.push(action);
                    assert_eq!(
                        next_world.project().booleans,
                        next_ioa.booleans,
                        "action {action} did not commute with IOA after {next_trace:?} ({kind:?})"
                    );
                    if visited.insert(next_ioa.booleans.clone()) {
                        queue.push_back((next_ioa, next_world, next_trace));
                    }
                }
            }
        }

        // All 5 non-redundant axes (3 membership states × 2 session_inputs_clean × 2 stream_inputs_clean
        // × 2 session_protected × 2 stream_protected = 48 states) are orthogonal and reachable.
        assert_eq!(
            visited.len(),
            48,
            "expected all 48 reachable states ({kind:?})"
        );
    }
}

#[test]
fn non_prose_metadata_edits_and_session_work_remain_orthogonal_to_stream_admission() {
    let mut world = ConcreteWorld::new(StreamProtectionKind::InheritedDefaultFlip);
    let baseline = world.project().booleans;

    // Concurrent non-prose edits (archive flag, issue links, explicit Automatic title_mode pin)
    // leave admission booleans unchanged and do not cause Conflict on automatic reports.
    world.session.title_mode = TitleMode::Automatic;
    world.stream_a.title_mode = TitleMode::Automatic;
    world.stream_a.status = Status::Closed;
    world.stream_a.references.push(
        Reference::new(
            ReferenceValue {
                name: "Issue 1".into(),
                purpose: "Supply context".into(),
                role: ReferenceRole::Background,
                state: ReferenceState::Active,
                target: ReferenceTarget::Issue {
                    url: "https://example.com/issue/1".into(),
                },
            },
            "fixture",
        )
        .unwrap(),
    );
    assert_eq!(world.project().booleans, baseline);

    let Reconciliation::Changes { record, .. } = world.session_report.apply(
        &world.session,
        world.current_stream(),
        world.current_defaults.session,
    ) else {
        panic!("expected SessionEdit to succeed across Inherit -> Automatic title_mode pin");
    };
    let Record::Session(updated_session) = *record else {
        unreachable!()
    };
    assert_eq!(updated_session.title_mode, TitleMode::Automatic);
    assert_eq!(updated_session.title, "Generated session title");

    let Reconciliation::Changes { record, .. } = world.stream_report.apply(
        &world.stream_a,
        Some(&world.session),
        world.current_defaults.stream,
    ) else {
        panic!("expected StreamEdit to succeed across non-prose metadata edits");
    };
    let Record::Stream(updated_stream) = *record else {
        unreachable!()
    };
    assert_eq!(updated_stream.status, Status::Closed);
    assert_eq!(updated_stream.title_mode, TitleMode::Automatic);
    assert_eq!(updated_stream.references.len(), 1);
    assert_eq!(
        updated_stream.goal.as_deref(),
        Some("Generated stream goal")
    );

    // Dirtying Session-local work invalidates SessionEdit but leaves StreamEdit eligible.
    world.apply_env_action("DirtySessionInputs");
    let ctx = world.project();
    assert!(!ctx.booleans["session_inputs_clean"]);
    assert!(ctx.booleans["membership_unchanged"] && ctx.booleans["stream_inputs_clean"]);
    assert_eq!(
        world
            .session_report
            .apply(
                &world.session,
                world.current_stream(),
                world.current_defaults.session
            )
            .outcome(),
        DomainOutcome::Superseded
    );
    assert_eq!(
        world
            .stream_report
            .apply(
                &world.stream_a,
                Some(&world.session),
                world.current_defaults.stream
            )
            .outcome(),
        DomainOutcome::Changes
    );
}

#[test]
fn removing_each_admission_conjunct_produces_formal_counterexample() {
    for (needle, replacement, expected_status, violated_flag, expected_value) in [
        (
            "membership_unchanged && stream_inputs_clean && !stream_protected",
            "in_captured_stream && stream_inputs_clean && !stream_protected",
            "StreamAdmitted",
            "membership_unchanged",
            false,
        ),
        (
            "membership_unchanged && stream_inputs_clean && !stream_protected",
            "membership_unchanged && !stream_protected",
            "StreamAdmitted",
            "stream_inputs_clean",
            false,
        ),
        (
            "membership_unchanged && stream_inputs_clean && !stream_protected",
            "membership_unchanged && stream_inputs_clean",
            "StreamAdmitted",
            "stream_protected",
            true,
        ),
        (
            "session_inputs_clean && !session_protected",
            "!session_protected",
            "SessionAdmitted",
            "session_inputs_clean",
            false,
        ),
        (
            "session_inputs_clean && !session_protected",
            "session_inputs_clean",
            "SessionAdmitted",
            "session_protected",
            true,
        ),
    ] {
        let mutated = REPORT_ADMISSION_IOA.replacen(needle, replacement, 1);
        assert_ne!(mutated, REPORT_ADMISSION_IOA, "mutation must alter spec");
        let report = VerificationCascade::from_ioa(&mutated)
            .with_sim_seeds(3)
            .with_prop_test_cases(50)
            .run();
        assert!(
            !report.all_passed,
            "mutated spec `{needle}` -> `{replacement}` unexpectedly passed"
        );
        let model_check = report
            .level_result(CascadeLevel::ModelCheck)
            .expect("model check level");
        assert!(
            !model_check.passed,
            "Stateright model check should catch `{needle}` -> `{replacement}`"
        );
        let counterexamples = &model_check.verification.as_ref().unwrap().counterexamples;
        assert!(
            counterexamples.iter().any(|ce| {
                ce.trace.last().is_some_and(|(state, _)| {
                    state.status == expected_status
                        && state.booleans.get(violated_flag) == Some(&expected_value)
                })
            }),
            "expected Stateright counterexample ending in {expected_status} with {violated_flag}={expected_value}, got: {counterexamples:#?}"
        );
    }
}

#[test]
fn naive_stream_id_comparison_breaks_conformance_after_return_move() {
    let table = report_admission_table();
    let mut ioa_ctx = EvalContext {
        booleans: table.initial_values.booleans.clone(),
        ..EvalContext::default()
    };
    let mut world = ConcreteWorld::new(StreamProtectionKind::TitleModeManual);

    for action in ["MoveToOtherStream", "ReturnToCapturedStream"] {
        ioa_ctx = step_ioa(table, &ioa_ctx, action).expect("valid move step");
        world.apply_env_action(action);
    }

    // Naive check that only compares current stream_id to captured stream_id misses A -> B -> A.
    let naive_same_stream_only =
        world.session.stream_id == world.stream_report.captured_session().unwrap().stream_id;
    assert!(naive_same_stream_only);

    // Production projection and IOA transition table both record the changed period and reject.
    let projected = world.project();
    assert_eq!(projected.booleans, ioa_ctx.booleans);
    assert!(projected.booleans["in_captured_stream"]);
    assert!(!projected.booleans["membership_unchanged"]);
    assert!(!report_admission_allows("AdmitStreamReport", &projected));
    assert_eq!(
        world
            .stream_report
            .apply(
                &world.stream_a,
                Some(&world.session),
                world.current_defaults.stream
            )
            .outcome(),
        DomainOutcome::Superseded
    );
    assert_eq!(
        world
            .session_report
            .apply(
                &world.session,
                &world.stream_a,
                world.current_defaults.session
            )
            .outcome(),
        DomainOutcome::Changes
    );
}
