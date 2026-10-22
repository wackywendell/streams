//! Negative model checks need a reachable witness, not a complete graph audit.
use stateright::{Checker, HasDiscoveries, Model};
use temper_verify::checker::Counterexample;

pub fn invariant_counterexample(source: &str) -> Option<Counterexample> {
    let model = temper_verify::build_model_from_ioa(source, 2).unwrap();
    // Keep the positive cascade's model and bound. Stop only after an invariant
    // violation, not a state/time limit or an unrelated property discovery.
    let checker = model
        .checker()
        .finish_when(HasDiscoveries::AllOf(["Invariants"].into()))
        .spawn_bfs()
        .join();
    checker.discovery("Invariants").map(|path| Counterexample {
        property: "Invariants".into(),
        trace: path.into_vec(),
    })
}
