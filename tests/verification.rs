use streams_temper::sync::protocol::{CATALOG_PROTOCOL_IOA, protocol_table, verify_total_context};
use temper_jit::table::EvalContext;
use temper_verify::cascade::VerificationCascade;

#[test]
fn projection_must_be_total() {
    let table = protocol_table();
    assert!(verify_total_context(table, &EvalContext::default()).is_err());
}

#[test]
fn catalog_protocol_passes_temper_cascade() {
    let report = VerificationCascade::from_ioa(CATALOG_PROTOCOL_IOA)
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
fn missing_save_marker_effect_has_counterexample() {
    let broken = CATALOG_PROTOCOL_IOA.replacen(
        r#"effect = ["save_marker = true", "adopting_marker = false", "candidate_reusable = false"]"#,
        r#"effect = ["save_marker = false", "adopting_marker = false", "candidate_reusable = false"]"#, 1);
    assert_ne!(broken, CATALOG_PROTOCOL_IOA);
    let report = VerificationCascade::from_ioa(&broken)
        .with_sim_seeds(2)
        .with_prop_test_cases(20)
        .run();
    assert!(!report.all_passed);
    let model = report
        .levels
        .iter()
        .find_map(|l| l.verification.as_ref())
        .unwrap();
    assert!(!model.counterexamples.is_empty(), "{report:#?}");
}
