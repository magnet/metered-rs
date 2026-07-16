use std::sync::atomic::{AtomicUsize, Ordering};

/// An OpenMetrics `stateset`: a set of mutually-exclusive states with exactly
/// one active, the live source of truth for that state.
///
/// Each state is exported as `name{name="state"} 0|1`. Use it for enum-like
/// status (e.g. `starting` / `running` / `draining` / `stopped`).
#[derive(Debug)]
pub struct StateSet {
    states: Vec<String>,
    active: AtomicUsize,
}

impl StateSet {
    /// Builds a state set over the given states; the first is active initially.
    pub fn new<I, S>(states: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        StateSet {
            states: states.into_iter().map(Into::into).collect(),
            active: AtomicUsize::new(0),
        }
    }

    /// Sets the active state by name, returning `false` if it is not a member.
    pub fn set(&self, state: &str) -> bool {
        match self.states.iter().position(|s| s == state) {
            Some(index) => {
                self.active.store(index, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// The currently active state, if any.
    pub fn current(&self) -> Option<&str> {
        self.states
            .get(self.active.load(Ordering::Relaxed))
            .map(String::as_str)
    }

    pub(crate) fn states(&self) -> &[String] {
        &self.states
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stateset_tracks_the_active_state() {
        let state = StateSet::new(["starting", "running", "stopped"]);
        assert_eq!(state.current(), Some("starting"));
        assert!(state.set("running"));
        assert_eq!(state.current(), Some("running"));
        assert!(!state.set("not-a-state"));
        assert_eq!(state.current(), Some("running"));
    }
}
