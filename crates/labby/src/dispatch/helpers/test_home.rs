//! The test-only home override must not cross unrelated concurrent tests.

use super::{TestLabHomeGuard, lab_home};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

#[test]
fn concurrent_test_home_guards_do_not_share_paths() {
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2)
        .map(|i| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let expected = PathBuf::from(format!("/isolated-fixture-{i}"));
                let _guard = TestLabHomeGuard::set(expected.clone());
                barrier.wait();
                let observed = lab_home();
                barrier.wait();
                (expected, observed)
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    for (expected, observed) in results {
        assert_eq!(
            observed, expected,
            "one test must not read another test's home"
        );
    }
}

#[test]
#[allow(clippy::panic)]
fn nested_test_home_guards_restore_after_unwind() {
    let outer = PathBuf::from("/outer-test-home");
    let _guard = TestLabHomeGuard::set(outer.clone());
    let result = std::panic::catch_unwind(|| {
        let _inner = TestLabHomeGuard::set(PathBuf::from("/inner-test-home"));
        assert_eq!(lab_home(), PathBuf::from("/inner-test-home"));
        panic!("exercise cleanup");
    });
    assert!(result.is_err());
    assert_eq!(lab_home(), outer);
}
