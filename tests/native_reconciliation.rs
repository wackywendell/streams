//! Proposed native lifecycle contract only. No Herdr/Pi service is contacted.
use crate::support::model::invariant_counterexample;
use streams_temper::sync::protocol::verify_total_context;
use temper_jit::table::{Effect, EvalContext, TransitionTable, effect_args};
use temper_verify::cascade::VerificationCascade;

const IOA: &str = include_str!("../specs/native_reconciliation.ioa.toml");

// A trace driver for Temper's evaluator, not a second protocol or model checker.
// The cascade below owns exploration, invariant checking and counterexamples.
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
                effect => panic!("unexpected model effect: {effect:?}"),
            }
        }
        self.phase = result.new_state;
    }
    fn adopt(&mut self) {
        self.step("AttestUnderLocks");
        self.step("CommitAdoption");
        assert_eq!(self.phase, "Active");
    }
    fn flag(&self, name: &str) -> bool {
        self.context.booleans[name]
    }
}

#[test]
fn proposed_reconciliation_passes_all_four_temper_levels() {
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
fn external_changes_are_admissible_even_during_adoption() {
    for phase in ["Observing", "Candidate", "Active"] {
        let mut t = Trace::new();
        if phase != "Observing" {
            t.step("AttestUnderLocks");
        }
        if phase == "Active" {
            t.step("CommitAdoption");
        }
        for rule in &t.table.rules {
            if rule.name.starts_with("External") {
                assert!(t.accepts(&rule.name), "{} rejected in {phase}", rule.name);
            }
        }
    }
}

#[test]
fn moves_unknowns_and_cas_races_are_recoverable_inputs_not_terminal_errors() {
    let mut t = Trace::new();
    t.adopt();
    t.step("ExternalOwnerMove"); // includes a new workspace-qualified pane ID
    assert!(!t.accepts("FocusExistingCard"));
    t.step("AttestUnderLocks");
    t.step("ExternalCasLost");
    assert!(!t.accepts("CommitAdoption"));
    t.adopt();
    t.step("ExternalUnavailable");
    assert!(!t.accepts("AttestUnderLocks"));
    assert!(!t.flag("card_closed"));
    t.step("ReattestSurvivingEndpoint");
    t.adopt();
    t.step("ExternalTreeNavigation");
    assert!(t.flag("same_conversation"));
    assert!(!t.accepts("FocusExistingCard"));
    t.adopt();
}

#[test]
fn destination_conflict_and_split_arrangement_never_steal_ownership() {
    for event in ["ExternalDestinationClaimed", "ExternalSplitArrangement"] {
        let mut t = Trace::new();
        t.step("AttestUnderLocks");
        t.step(event); // admissible while locked/candidate, not a protocol violation
        assert!(!t.accepts("CommitAdoption"));
        assert!(!t.accepts("AttestUnderLocks"));
        t.step(if event == "ExternalDestinationClaimed" {
            "ExternalDestinationReleased"
        } else {
            "ExternalArrangementReunited"
        });
        t.adopt();
    }
}

#[test]
fn independent_resource_moves_remain_owned_without_duplicate_or_automatic_return() {
    let mut t = Trace::new();
    t.adopt();
    t.step("ExternalCardMove");
    t.adopt();
    assert!(t.flag("card_present") && t.flag("card_separated"));
    assert!(!t.accepts("ExplicitReopenCard"));
    assert!(!t.accepts("FocusExistingCard"));
    assert!(t.accepts("FocusExistingEditor"));
    t.step("ExternalEditorMove");
    t.adopt();
    assert!(t.flag("editor_present") && t.flag("editor_separated"));
    assert!(!t.accepts("ExplicitReopenEditor"));
    assert!(!t.accepts("FocusExistingEditor"));
    t.step("ExternalCardReturn");
    t.step("ExternalEditorReturn");
    t.adopt();
    t.step("FocusExistingCard");
    t.step("FocusExistingEditor");
}

#[test]
fn independent_closures_need_positive_absence_and_an_explicit_reopen() {
    for resource in ["Card", "Editor"] {
        let mut t = Trace::new();
        t.adopt();
        t.step(&format!("External{resource}Close"));
        t.step("ExternalUnavailable");
        assert!(!t.accepts(&format!("Confirm{resource}Closure")));
        t.step("ReattestSurvivingEndpoint");
        t.adopt();
        assert!(!t.accepts(&format!("ExplicitReopen{resource}")));
        t.step("ExternalOwnerMove");
        t.step(&format!("Confirm{resource}Closure"));
        t.adopt();
        // Remaining closed is stable: adoption did not respawn anything.
        let key = resource.to_lowercase();
        assert!(t.flag(&format!("{key}_closed")));
        t.step(&format!("ExplicitReopen{resource}"));
        assert!(!t.accepts(&format!("ExplicitReopen{resource}")));
        t.step(&format!("Acknowledge{resource}Reopen"));
        t.adopt();
        assert!(t.flag(&format!("{key}_present")));
        assert!(!t.accepts(&format!("ExplicitReopen{resource}")));
    }
}

#[test]
fn in_flight_reopen_receipts_cannot_cross_an_external_interruption() {
    for resource in ["Card", "Editor"] {
        let mut events = vec![
            "ExternalOwnerMove".to_string(),
            "ExternalCasLost".to_string(),
            "ExternalUnavailable".to_string(),
            "ExternalRestartOrRestore".to_string(),
            "ExternalDestinationClaimed".to_string(),
            "ExternalSplitArrangement".to_string(),
            "ExternalRecycledHandle".to_string(),
            format!("External{resource}Move"),
            format!("External{resource}Return"),
            format!("External{resource}Close"),
            format!("External{resource}Reappears"),
        ];
        if resource == "Editor" {
            events.push("ExternalTreeNavigation".into());
        }
        for event in events {
            let mut t = Trace::new();
            t.step(&format!("External{resource}Close"));
            t.step(&format!("Confirm{resource}Closure"));
            t.adopt();
            t.step(&format!("ExplicitReopen{resource}"));
            let key = resource.to_lowercase();
            assert!(t.flag(&format!("{key}_pending")));
            assert!(!t.flag(&format!("{key}_uncertain")));
            t.step(&event);
            // Repairing owner observation/ownership cannot repair a lost receipt context.
            if t.accepts("ReattestSurvivingEndpoint") {
                t.step("ReattestSurvivingEndpoint");
            }
            t.step("ExternalDestinationReleased");
            t.step("ExternalArrangementReunited");
            assert!(t.flag(&format!("{key}_pending")), "{event}");
            assert!(t.flag(&format!("{key}_uncertain")), "{event}");
            assert!(
                !t.accepts(&format!("Acknowledge{resource}Reopen")),
                "{event}"
            );
            assert!(!t.accepts("AttestUnderLocks"), "{event}");
        }
    }
}

#[test]
fn uncertainty_survives_moves_closures_reappearance_and_restart() {
    for resource in ["Card", "Editor"] {
        let mut t = Trace::new();
        t.step(&format!("External{resource}LaunchUncertain"));
        for event in [
            "ExternalOwnerMove".to_string(),
            format!("External{resource}Move"),
            format!("External{resource}Close"),
            "ExternalRestartOrRestore".to_string(),
        ] {
            t.step(&event);
        }
        t.step("ReattestSurvivingEndpoint");
        t.step(&format!("Confirm{resource}Closure"));
        assert!(!t.accepts("AttestUnderLocks"));
        assert!(!t.accepts(&format!("Acknowledge{resource}Reopen")));
        t.step(&format!("External{resource}Reappears"));
        assert!(t.flag(&format!("{}_pending", resource.to_lowercase())));
        assert!(!t.accepts(&format!("ExplicitReopen{resource}")));
    }
}

#[test]
fn manual_names_forks_and_recycled_handles_are_not_adoption_shortcuts() {
    let mut t = Trace::new();
    t.step("ExternalRename");
    t.step("ExternalOwnerMove");
    t.adopt();
    assert!(t.flag("manual_name"));
    assert!(!t.accepts("ProjectAutomaticName"));
    t.step("ExplicitNameReclaim");
    t.step("ProjectAutomaticName");
    t.step("ExternalRestartOrRestore");
    assert!(!t.accepts("AttestUnderLocks"));
    t.step("ReattestSurvivingEndpoint");
    t.adopt();
    t.step("ExternalRecycledHandle");
    assert!(!t.accepts("ReattestSurvivingEndpoint"));
    assert!(!t.accepts("AttestUnderLocks"));
    t.step("ExternalForkOrSwitch");
    assert_eq!(t.phase, "Retired");
    assert!(!t.accepts("AttestUnderLocks"));
    assert!(!t.accepts("AcknowledgeCardReopen"));
    assert!(!t.accepts("FocusExistingEditor"));
}

#[test]
fn committed_fork_retires_the_lane_without_erasing_either_pending_attempt() {
    let mut t = Trace::new();
    t.step("ExternalCardLaunchUncertain");
    t.step("ExternalEditorLaunchUncertain");
    t.step("ExternalForkOrSwitch");
    assert_eq!(t.phase, "Retired");
    assert!(t.flag("card_pending") && t.flag("editor_pending"));
    // New identity/enrollment needs a separate model instance, not old authority.
    for rule in &t.table.rules {
        assert!(!t.accepts(&rule.name));
    }
}

fn assert_counterexample(old: &str, new: &str) {
    let broken = IOA.replacen(old, new, 1);
    assert_ne!(broken, IOA);
    assert!(
        invariant_counterexample(&broken).is_some(),
        "mutation must yield a reachable invariant counterexample: {old}"
    );
}

#[test]
fn unguarded_manual_name_projection_has_counterexample() {
    assert_counterexample("guard = \"!manual_name\"", "guard = \"true\"");
}

#[test]
fn owner_move_clearing_pending_attempt_has_counterexample() {
    assert_counterexample(
        "effect = [\"fresh = false\", \"destination_clear = false\", \"card_uncertain = card_pending\", \"editor_uncertain = editor_pending\"]",
        "effect = [\"fresh = false\", \"destination_clear = false\", \"card_pending = false\"]",
    );
}

#[test]
fn external_change_keeping_active_projection_has_counterexample() {
    assert_counterexample(
        "to = \"Observing\"\nguard = \"true\"\neffect = [\"fresh = false\", \"card_uncertain = card_pending\", \"editor_uncertain = editor_pending\"]",
        "to = \"Active\"\nguard = \"true\"\neffect = [\"fresh = false\", \"card_uncertain = card_pending\", \"editor_uncertain = editor_pending\"]",
    );
}

#[test]
fn acknowledging_uncertain_reopen_has_counterexample() {
    assert_counterexample(
        "guard = \"card_pending && !card_uncertain && !card_present && same_terminal && same_conversation && endpoint_verified && destination_clear && relocation_safe\"",
        "guard = \"card_pending && !card_present && same_terminal && same_conversation && endpoint_verified && destination_clear && relocation_safe\"",
    );
}
