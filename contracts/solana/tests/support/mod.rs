mod actions;
mod fixture;
mod transport;

pub use fixture::Fixture;
pub use transport::{ix, send, send_logs, Role};

pub fn rejects(result: Result<(), String>, expected: &str) {
    let error = result.expect_err("instruction must reject");
    assert!(error.contains(expected), "expected {expected}: {error}");
}
