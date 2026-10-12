// SPDX-License-Identifier: GPL-3.0-or-later
use super::config::ReadyLayout;
use std::sync::{Arc, Mutex};

#[derive(Default)]
enum Phase {
    #[default]
    Offline,
    Resolving,
    Ready(Arc<ReadyLayout>),
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout() -> ReadyLayout {
        ReadyLayout::fixture(serde_json::from_str(r#"{"guild":1,"category":2,"lobby":3}"#).unwrap())
    }
    #[test]
    fn stale_resolution_cannot_restore_a_disconnected_session() {
        let session = Session::default();
        let old = session.begin();
        session.disconnect();
        session.finish(old, layout());
        assert!(session.lease().is_none());
        let current = session.begin();
        session.finish(current, layout());
        let lease = session.lease().unwrap();
        assert!(session.current(&lease));
        session.disconnect();
        assert!(!session.current(&lease));
    }
    #[test]
    fn only_latest_resolution_can_publish_readiness() {
        let session = Session::default();
        let old = session.begin();
        let current = session.begin();
        session.finish(old, layout());
        assert!(session.lease().is_none());
        session.finish(current, layout());
        assert!(session.lease().is_some());
    }
}
#[derive(Default)]
struct State {
    revision: u64,
    phase: Phase,
}
#[derive(Default)]
pub struct Session(Mutex<State>);
pub struct Resolution(u64);
#[derive(Clone)]
pub struct Lease {
    revision: u64,
    layout: Arc<ReadyLayout>,
}
impl Lease {
    pub fn layout(&self) -> &ReadyLayout {
        &self.layout
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
}
impl Session {
    pub fn disconnect(&self) {
        let mut s = self.0.lock().expect("session lock poisoned");
        s.revision += 1;
        s.phase = Phase::Offline;
    }
    pub fn begin(&self) -> Resolution {
        let mut s = self.0.lock().expect("session lock poisoned");
        s.revision += 1;
        s.phase = Phase::Resolving;
        Resolution(s.revision)
    }
    pub fn finish(&self, attempt: Resolution, layout: ReadyLayout) {
        let mut s = self.0.lock().expect("session lock poisoned");
        if s.revision == attempt.0 && matches!(s.phase, Phase::Resolving) {
            s.phase = Phase::Ready(Arc::new(layout));
        }
    }
    pub fn lease(&self) -> Option<Lease> {
        let s = self.0.lock().expect("session lock poisoned");
        match &s.phase {
            Phase::Ready(layout) => Some(Lease {
                revision: s.revision,
                layout: layout.clone(),
            }),
            _ => None,
        }
    }
    pub fn current(&self, lease: &Lease) -> bool {
        let s = self.0.lock().expect("session lock poisoned");
        s.revision == lease.revision && matches!(s.phase, Phase::Ready(_))
    }
}
