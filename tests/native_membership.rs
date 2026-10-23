//! Proposed membership/result admission contract. No native service or live catalog is used.
use streams_temper::sync::protocol::verify_total_context;
use temper_jit::table::{Effect, EvalContext, TransitionTable, effect_args};
use temper_verify::cascade::VerificationCascade;

const IOA: &str = include_str!("../specs/native_membership.ioa.toml");

// Scenario runner over Temper's evaluator; exploration and invariant checking
// belong to the verification cascade, not to this helper.
struct Trace {
    table: TransitionTable,
    phase: String,
    context: EvalContext,
}
impl Trace {
    fn new() -> Self {
        let table = TransitionTable::try_from_ioa_source(IOA).unwrap();
        let context = EvalContext {
            booleans: table.initial_values.booleans.clone(),
            ..EvalContext::default()
        };
        verify_total_context(&table, &context).unwrap();
        Self {
            phase: table.initial_state.clone(),
            table,
            context,
        }
    }
    fn captured() -> Self {
        let mut trace = Self::new();
        trace.step("AttestCaller");
        trace.step("CaptureContext");
        trace
    }
    fn accepts(&self, action: &str) -> bool {
        verify_total_context(&self.table, &self.context).unwrap();
        self.table
            .evaluate_ctx(&self.phase, &self.context, action)
            .is_some_and(|result| result.success)
    }
    fn step(&mut self, action: &str) {
        assert!(self.accepts(action), "{action} rejected in {}", self.phase);
        let result = self
            .table
            .evaluate_ctx(&self.phase, &self.context, action)
            .unwrap();
        for effect in result.effects {
            match effect {
                Effect::SetState(_) => {}
                Effect::SetBool { var, value } => {
                    let value = effect_args::boolean(
                        &value,
                        &self.context.booleans,
                        &serde_json::Value::Null,
                    )
                    .unwrap();
                    *self.context.booleans.get_mut(&var).unwrap() = value;
                }
                other => panic!("unexpected model effect: {other:?}"),
            }
        }
        self.phase = result.new_state;
    }
    fn flag(&self, name: &str) -> bool {
        self.context.booleans[name]
    }
    fn move_to(&mut self, destination: &str) {
        self.step(destination);
        assert!(!self.accepts("CommitMembership"));
        self.step("AttestCaller");
        if self.flag("workspace_b") && !self.flag("destination_exists") {
            self.step("CreateAndBindDestination");
        }
        self.step("CommitMembership");
    }
}

#[test]
fn membership_admission_passes_all_four_temper_levels() {
    let report = VerificationCascade::from_ioa(IOA)
        .with_sim_seeds(5)
        .with_prop_test_cases(100)
        .run();
    for level in &report.levels {
        eprintln!("{} {:?}", level.summary, level.diagnostics);
    }
    assert_eq!(report.levels.len(), 4);
    assert!(report.all_passed, "{report:#?}");
}

#[test]
fn environment_changes_remain_valid_while_results_are_pending() {
    for trace in [Trace::new(), Trace::captured()] {
        for rule in &trace.table.rules {
            if rule.name.starts_with("External") {
                assert!(
                    trace.accepts(&rule.name),
                    "{} rejected in {}",
                    rule.name,
                    trace.phase
                );
            }
        }
    }
}

#[test]
fn only_the_moving_conversation_follows_the_newly_associated_workspace() {
    for peer_b in [false, true] {
        let mut trace = Trace::captured();
        if peer_b {
            trace.step("ExternalPeerToB");
        }
        trace.step("ExternalNativeAttemptUncertain");
        trace.step("ExternalMoveToB");
        assert!(!trace.accepts("AcceptWork"));
        trace.step("AttestCaller");
        assert!(!trace.accepts("CommitMembership"));
        trace.step("CreateAndBindDestination");
        trace.step("CommitMembership");
        assert!(trace.flag("member_b") && trace.flag("visited_b"));
        assert_eq!(trace.flag("peer_member_b"), peer_b);
        assert!(trace.accepts("AcceptWork"));
        assert!(!trace.accepts("AcceptStream"));
        trace.step("AcceptWork");
        assert!(!trace.flag("work_pending") && trace.flag("stream_pending"));
        assert!(trace.flag("native_pending") && trace.flag("native_uncertain"));
    }
}

#[test]
fn returning_to_the_same_stream_does_not_revive_an_old_membership_context() {
    let mut trace = Trace::captured();
    trace.move_to("ExternalMoveToB");
    trace.move_to("ExternalMoveToA");
    assert_eq!(trace.flag("member_b"), trace.flag("captured_member_b"));
    assert!(trace.flag("visited_b"));
    assert!(!trace.flag("membership_current"));
    assert!(trace.accepts("AcceptWork"));
    assert!(!trace.accepts("AcceptStream"));
    trace.step("AcceptWork");
    trace.step("DiscardRemainingProposals");
    trace.step("CaptureContext");
    assert!(trace.accepts("AcceptWork") && trace.accepts("AcceptStream"));
}

#[test]
fn work_and_stream_conflicts_are_independent_but_tree_changes_expire_both() {
    for (event, work, stream) in [
        ("ExternalWorkConflict", false, true),
        ("ExternalStreamConflict", true, false),
        ("ExternalPeerToB", true, false),
        ("ExternalTreeNavigation", false, false),
    ] {
        let mut trace = Trace::captured();
        trace.step(event);
        assert_eq!(trace.accepts("AcceptWork"), work, "{event}");
        assert_eq!(trace.accepts("AcceptStream"), stream, "{event}");
    }
}

#[test]
fn proposals_are_consumed_once_until_a_fresh_context() {
    let mut trace = Trace::captured();
    trace.step("AcceptWork");
    assert!(!trace.accepts("AcceptWork") && trace.accepts("AcceptStream"));
    trace.step("AcceptStream");
    assert!(!trace.accepts("AcceptWork") && !trace.accepts("AcceptStream"));
    trace.step("DiscardRemainingProposals");
    trace.step("CaptureContext");
    assert!(trace.accepts("AcceptWork") && trace.accepts("AcceptStream"));
}

#[test]
fn transient_observation_loss_recovers_without_replacing_the_context() {
    let mut trace = Trace::captured();
    trace.step("ExternalUnavailable");
    assert!(!trace.accepts("AttestCaller") && !trace.accepts("AcceptWork"));
    trace.step("ExternalObservationReturns");
    assert!(!trace.accepts("AcceptWork"));
    trace.step("AttestCaller");
    assert!(trace.accepts("AcceptWork") && trace.accepts("AcceptStream"));
}

#[test]
fn foreign_destinations_are_not_stolen_and_do_not_retarget_old_stream_results() {
    let mut trace = Trace::captured();
    trace.step("ExternalMoveToB");
    trace.step("ExternalDestinationClaimed");
    trace.step("AttestCaller");
    assert!(!trace.accepts("CreateAndBindDestination"));
    assert!(!trace.accepts("CommitMembership"));
    assert!(trace.accepts("AcceptWork"));
    assert!(!trace.accepts("AcceptStream"));
    trace.step("ExternalDestinationOwnershipReturns");
    trace.step("CommitMembership");
    assert!(!trace.accepts("AcceptStream"));
}

#[test]
fn source_authority_loss_cannot_be_bypassed_by_an_available_destination() {
    let mut trace = Trace::captured();
    trace.step("ExternalSourceOwnershipLost");
    trace.step("ExternalMoveToB");
    trace.step("AttestCaller");
    assert!(!trace.accepts("CreateAndBindDestination"));
    assert!(!trace.accepts("CommitMembership"));
    assert!(!trace.accepts("AcceptStream"));
    trace.step("ExternalSourceOwnershipReturns");
    trace.step("CreateAndBindDestination");
    trace.step("CommitMembership");
    assert!(!trace.accepts("AcceptStream"));
}

#[test]
fn a_committed_fork_retires_old_proposals_without_erasing_native_attempts() {
    let mut trace = Trace::captured();
    trace.step("ExternalNativeAttemptUncertain");
    trace.step("ExternalForkOrReplacement");
    assert_eq!(trace.phase, "Retired");
    assert!(trace.flag("native_pending") && trace.flag("native_uncertain"));
    for rule in &trace.table.rules {
        assert!(!trace.accepts(&rule.name));
    }
    // A new conversation needs its own identity/card and fresh context, not reuse.
    let fresh = Trace::new();
    assert!(!fresh.flag("work_pending") && !fresh.flag("caller_verified"));
}

#[test]
fn weakened_scope_or_preservation_guards_produce_temper_counterexamples() {
    const COMMIT_GUARD: &str = "guard = \"caller_verified && source_owned && destination_owned && member_b != workspace_b && (!workspace_b || destination_exists)\"";
    for (old, new, actions) in [
        (
            COMMIT_GUARD,
            "guard = \"source_owned && destination_owned && member_b != workspace_b && (!workspace_b || destination_exists)\"",
            &[
                "AttestCaller",
                "ExternalMoveToB",
                "CreateAndBindDestination",
                "CommitMembership",
            ][..],
        ),
        (
            COMMIT_GUARD,
            "guard = \"caller_verified && destination_owned && member_b != workspace_b && (!workspace_b || destination_exists)\"",
            &[
                "AttestCaller",
                "ExternalMoveToB",
                "CreateAndBindDestination",
                "ExternalSourceOwnershipLost",
                "CommitMembership",
            ][..],
        ),
        (
            COMMIT_GUARD,
            "guard = \"caller_verified && source_owned && member_b != workspace_b && (!workspace_b || destination_exists)\"",
            &[
                "AttestCaller",
                "ExternalMoveToB",
                "ExternalDestinationClaimed",
                "CommitMembership",
            ][..],
        ),
        (
            COMMIT_GUARD,
            "guard = \"caller_verified && source_owned && destination_owned && (!workspace_b || destination_exists)\"",
            &["AttestCaller", "CommitMembership"][..],
        ),
        (
            "guard = \"caller_verified && workspace_b && !destination_exists && destination_owned && source_owned\"",
            "guard = \"true\"",
            &["CreateAndBindDestination"][..],
        ),
        (
            "guard = \"caller_verified && member_b == workspace_b && ((member_b && destination_exists && destination_owned) || (!member_b && source_owned))\"",
            "guard = \"true\"",
            &["CaptureContext"][..],
        ),
        (
            "guard = \"work_pending && caller_verified && branch_current && work_base_current\"",
            "guard = \"caller_verified && branch_current && work_base_current\"",
            &["AttestCaller", "CaptureContext", "AcceptWork"][..],
        ),
        (
            "guard = \"stream_pending && caller_verified",
            "guard = \"caller_verified",
            &["AttestCaller", "CaptureContext", "AcceptStream"][..],
        ),
        (
            "\"work_consumed = true\", \"work_pending = false\"",
            "\"work_consumed = true\"",
            &["AttestCaller", "CaptureContext", "AcceptWork"][..],
        ),
        (
            "\"stream_consumed = true\", \"stream_pending = false\"",
            "\"stream_consumed = true\"",
            &["AttestCaller", "CaptureContext", "AcceptStream"][..],
        ),
        (
            "\"visited_b = true\"",
            "\"visited_b = workspace_b\"",
            &[
                "AttestCaller",
                "ExternalMoveToB",
                "CreateAndBindDestination",
                "CommitMembership",
                "ExternalMoveToA",
            ][..],
        ),
        (
            "guard = \"work_pending && caller_verified && branch_current && work_base_current\"",
            "guard = \"work_pending && caller_verified && work_base_current\"",
            &[
                "AttestCaller",
                "CaptureContext",
                "ExternalTreeNavigation",
                "AcceptWork",
            ][..],
        ),
        (
            "&& membership_current && member_b == captured_member_b",
            "&& member_b == captured_member_b",
            &[
                "AttestCaller",
                "CaptureContext",
                "ExternalMoveToB",
                "CreateAndBindDestination",
                "CommitMembership",
                "ExternalMoveToA",
                "AcceptStream",
            ][..],
        ),
        (
            "\"member_b = workspace_b\", \"visited_b = true\"",
            "\"member_b = workspace_b\", \"visited_b = true\", \"peer_member_b = workspace_b\"",
            &[
                "AttestCaller",
                "ExternalMoveToB",
                "CreateAndBindDestination",
                "CommitMembership",
            ][..],
        ),
        (
            "\"member_b = workspace_b\", \"visited_b = true\"",
            "\"member_b = workspace_b\", \"visited_b = true\", \"native_pending = false\"",
            &[
                "AttestCaller",
                "ExternalMoveToB",
                "CreateAndBindDestination",
                "CommitMembership",
                "ExternalNativeAttemptUncertain",
            ][..],
        ),
    ] {
        let broken = IOA.replacen(old, new, 1);
        assert_ne!(broken, IOA);
        let mut model = temper_verify::model::build_model_from_ioa(&broken, 2).unwrap();
        // A counterexample using only these real actions is also a counterexample
        // in the full automaton. Avoid exhaustively rechecking unrelated events
        // for every mutation; the unmodified model runs the complete L0–L3 cascade.
        model
            .transitions
            .retain(|transition| actions.contains(&transition.name.as_str()));
        let report = temper_verify::checker::check_model(&model);
        assert!(
            !report.all_properties_hold,
            "mutation survived: {old} -> {new}"
        );
        assert!(
            !report.counterexamples.is_empty(),
            "expected a counterexample: {report:#?}"
        );
    }
}
