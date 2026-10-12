use std::sync::atomic::{AtomicU64, Ordering};

/// Keep the requested bit and its generation together so old workers cannot
/// clear a newer request, including Disconnect followed by Connect.
pub struct ConnectionRequest(AtomicU64);

impl ConnectionRequest {
    pub fn new(requested: bool) -> Self {
        Self(AtomicU64::new(u64::from(requested)))
    }

    pub fn load(&self, ordering: Ordering) -> bool {
        self.0.load(ordering) & 1 != 0
    }

    pub fn store(&self, requested: bool, ordering: Ordering) {
        let _ = self.0.fetch_update(ordering, Ordering::Relaxed, |token| {
            Some((token.wrapping_add(2) & !1) | u64::from(requested))
        });
    }

    pub fn snapshot(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    pub fn is_current(&self, token: u64) -> bool {
        self.snapshot() == token
    }

    pub fn claim(&self) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |token| {
                (token & 1 == 0).then(|| token.wrapping_add(2) | 1)
            })
            .is_ok()
    }

    pub fn release(&self, token: u64) -> bool {
        self.0
            .compare_exchange(
                token,
                token.wrapping_add(2) & !1,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};

    #[test]
    fn old_reclaim_completion_cannot_cancel_a_new_connect_or_repair() {
        for disconnect_first in [false, true] {
            let request = Arc::new(ConnectionRequest::new(true));
            let old = request.snapshot();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (finish_tx, finish_rx) = mpsc::channel();
            let worker = {
                let request = request.clone();
                std::thread::spawn(move || {
                    ready_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    request.release(old)
                })
            };
            ready_rx.recv().unwrap();
            if disconnect_first {
                request.store(false, Ordering::Release);
            }
            request.store(true, Ordering::Release);
            finish_tx.send(()).unwrap();
            assert!(!worker.join().unwrap());
            assert!(request.load(Ordering::Acquire));
            assert!(request.release(request.snapshot()));
            assert!(!request.load(Ordering::Acquire));
        }
    }

    #[test]
    fn automatic_request_has_its_own_generation_and_cannot_duplicate() {
        let request = ConnectionRequest::new(true);
        let old = request.snapshot();
        request.store(false, Ordering::Release);
        assert!(request.claim());
        assert!(!request.claim());
        assert!(!request.release(old));
        assert!(request.load(Ordering::Acquire));
    }
}
