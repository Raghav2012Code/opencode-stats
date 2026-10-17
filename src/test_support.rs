//! Helpers shared by the tests, compiled only for them.
//!
//! Tests that touch the disk need a path nothing else in the run can collide with. This
//! was written out five times before; here it is once.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A path under the temporary directory that is unique to this test run.
pub fn unique_temp_path(label: &str, suffix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is before the unix epoch")
        .as_nanos();

    std::env::temp_dir().join(format!("oc-stats-{label}-{nonce}{suffix}"))
}

/// A unique temporary directory, created and ready to write into.
pub fn unique_temp_dir(label: &str) -> PathBuf {
    let dir = unique_temp_path(label, "");
    std::fs::create_dir_all(&dir).expect("could not create a temporary directory");
    dir
}
