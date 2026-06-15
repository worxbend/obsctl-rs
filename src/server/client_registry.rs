use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Thread-safe counter for currently connected IPC clients.
#[derive(Clone, Default)]
pub struct ClientRegistry {
    count: Arc<AtomicUsize>,
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Register a new client and return a guard that decrements on drop.
    pub fn register(&self) -> ClientGuard {
        self.count.fetch_add(1, Ordering::Relaxed);
        ClientGuard {
            count: Arc::clone(&self.count),
        }
    }
}

pub struct ClientGuard {
    count: Arc<AtomicUsize>,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_drop() {
        let reg = ClientRegistry::new();
        assert_eq!(reg.count(), 0);
        let g1 = reg.register();
        assert_eq!(reg.count(), 1);
        let g2 = reg.register();
        assert_eq!(reg.count(), 2);
        drop(g1);
        assert_eq!(reg.count(), 1);
        drop(g2);
        assert_eq!(reg.count(), 0);
    }
}
