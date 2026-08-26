use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Tracks in-flight turn tokens keyed by session id.
///
/// A session may have several turns registered at once — the host can send a
/// second prompt for the same session before the first finishes, and although
/// the adapter mutex serializes execution it does not serialize registration.
/// Keying by session alone would let the second registration clobber the
/// first's token, so cancellation becomes unreliable. We keep a vec of tokens
/// per session instead.
#[derive(Clone, Default)]
pub struct CancelRegistry {
    inner: Arc<Mutex<HashMap<String, Vec<Arc<AtomicBool>>>>>,
}

impl CancelRegistry {
    /// Creates a fresh token, pushes it for this session, and returns it.
    /// Never overwrites an existing token.
    pub fn register(&self, session_id: &str) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        self.inner
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default()
            .push(Arc::clone(&token));
        token
    }

    /// Removes only the exact token, matched by pointer identity, never by
    /// position or by clearing the session (the other turns must survive).
    /// When the session's vec empties, drop the map entry so we don't leak
    /// one entry per session for the life of the process.
    pub fn unregister(&self, session_id: &str, token: &Arc<AtomicBool>) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(vec) = guard.get_mut(session_id) {
            vec.retain(|t| !Arc::ptr_eq(t, token));
            if vec.is_empty() {
                guard.remove(session_id);
            }
        }
    }

    /// Sets every token for the session. Cancelling a session means cancelling
    /// all of its in-flight work; a queued second turn should not outlive the
    /// user's cancel of the first.
    pub fn cancel(&self, session_id: &str) {
        if let Some(vec) = self.inner.lock().unwrap().get(session_id) {
            for token in vec {
                token.store(true, Ordering::SeqCst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_a_session_cancels_every_in_flight_turn() {
        let reg = CancelRegistry::default();
        let a = reg.register("s");
        let b = reg.register("s");
        reg.cancel("s");
        assert!(a.load(Ordering::SeqCst));
        assert!(b.load(Ordering::SeqCst));
    }

    #[test]
    fn a_finished_turn_does_not_unregister_another_turns_token() {
        let reg = CancelRegistry::default();
        let first = reg.register("s");
        let second = reg.register("s");
        reg.unregister("s", &first);
        reg.cancel("s");
        assert!(!first.load(Ordering::SeqCst));
        assert!(second.load(Ordering::SeqCst));
    }

    #[test]
    fn unregistering_the_last_token_removes_the_session_entry() {
        let reg = CancelRegistry::default();
        let token = reg.register("s");
        reg.unregister("s", &token);
        assert!(reg.inner.lock().unwrap().is_empty());
    }

    #[test]
    fn cancelling_an_unknown_session_is_a_no_op() {
        let reg = CancelRegistry::default();
        reg.cancel("nope");
    }

    #[test]
    fn tokens_for_different_sessions_are_independent() {
        let reg = CancelRegistry::default();
        let a = reg.register("a");
        let b = reg.register("b");
        reg.cancel("a");
        assert!(a.load(Ordering::SeqCst));
        assert!(!b.load(Ordering::SeqCst));
    }
}
