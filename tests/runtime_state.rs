//! The remaining host-local root only addresses directory bindings.
use streams_temper::runtime::state::RuntimeState;
#[test]
fn host_scoped_root_construction_is_inert_and_keeps_its_existing_address() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap().join("runtime");
    let state = RuntimeState::new(&base, "host").unwrap();
    let again = RuntimeState::new(&base, "host").unwrap();
    let other = RuntimeState::new(&base, "other").unwrap();
    assert_eq!(state.root(), again.root());
    assert_ne!(state.root(), other.root());
    assert_eq!(state.host_id(), "host");
    for host in ["", " ", "bad\0", "bad\n"] {
        assert!(RuntimeState::new(&base, host).is_err());
    }
    assert!(!base.exists());
}
