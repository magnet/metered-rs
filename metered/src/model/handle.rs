//! A cheaply-shareable handle to a metric's backing state.
//!
//! `Handle<T>` is an `Arc<T>` newtype shared by the metric primitives
//! (`Counter`, `Gauge`) and the legacy recording wrappers, and referenced by
//! macro-generated code as `metered::handle::Handle`.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// A cheaply-shareable handle to a metric's backing state.
#[doc(hidden)]
pub struct Handle<T> {
    inner: Arc<T>,
}

impl<T> Handle<T> {
    /// Wraps `value` in a new shared handle.
    #[doc(hidden)]
    pub fn new(value: T) -> Self {
        Handle {
            inner: Arc::new(value),
        }
    }

    /// Returns another handle to the same backing state.
    #[doc(hidden)]
    pub fn share(&self) -> Self {
        Handle {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        self.share()
    }
}

impl<T> Deref for Handle<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: Default> Default for Handle<T> {
    fn default() -> Self {
        Handle::new(T::default())
    }
}

impl<T: fmt::Debug> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use super::Handle;

    #[test]
    fn handle_shares_and_derefs_backing_state() {
        let handle = Handle::new(41u64);
        let shared = handle.share();

        assert_eq!(*handle, 41);
        assert_eq!(*shared, 41);
        assert_eq!(format!("{:?}", shared), "41");
    }
}
