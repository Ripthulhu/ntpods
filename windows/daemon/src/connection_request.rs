use std::sync::atomic::{AtomicU64, Ordering};

/// Keep control/audio intent and its generation together so stale cleanup cannot
/// clear a newer Connect. A passive Windows link authorizes controls only.
pub struct ConnectionRequest(AtomicU64);

#[derive(Clone)]
pub struct PairedTarget {
    pub address: u64,
    pub request: u64,
    pub stage: PairingStage,
}

#[derive(Clone)]
pub enum PairingStage {
    Validating,
    Ready(String),
    Selected,
}

impl PairedTarget {
    pub fn current(&self, connection: &ConnectionRequest) -> bool {
        connection.is_current(self.request)
    }
}

pub fn pairing_allows_selection(
    pending: Option<&PairedTarget>,
    connection: &ConnectionRequest,
    request: Option<u64>,
) -> bool {
    match request {
        Some(request) => pending.is_some_and(|target| {
            target.current(connection)
                && target.request == request
                && matches!(target.stage, PairingStage::Ready(_))
        }),
        None => pending.is_none_or(|target| !target.current(connection)),
    }
}

pub fn pairing_allows_connect(
    pending: Option<&PairedTarget>,
    connection: &ConnectionRequest,
    address: u64,
) -> bool {
    pending.is_none_or(|target| {
        !target.current(connection)
            || (matches!(target.stage, PairingStage::Selected) && target.address == address)
    })
}

impl ConnectionRequest {
    pub fn new(requested: bool) -> Self {
        Self(AtomicU64::new(if requested { 3 } else { 0 }))
    }

    pub fn load(&self, ordering: Ordering) -> bool {
        self.0.load(ordering) & 1 != 0
    }

    pub fn explicit(&self) -> bool {
        self.snapshot() & 3 == 3
    }

    pub fn may_open_controls(&self, token: u64, windows_connected: bool) -> bool {
        token & 1 != 0 && self.is_current(token) && (token & 2 != 0 || windows_connected)
    }

    pub fn store(&self, requested: bool, ordering: Ordering) {
        let _ = self.0.fetch_update(ordering, Ordering::Relaxed, |token| {
            Some((token.wrapping_add(4) & !3) | if requested { 3 } else { 0 })
        });
    }

    pub fn renew(&self) -> u64 {
        self.0.fetch_add(4, Ordering::AcqRel).wrapping_add(4)
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
                (token & 1 == 0).then(|| (token.wrapping_add(4) & !3) | 1)
            })
            .is_ok()
    }

    pub fn release(&self, token: u64) -> bool {
        self.0
            .compare_exchange(
                token,
                token.wrapping_add(4) & !3,
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
    fn old_session_completion_cannot_cancel_a_new_connect_or_repair() {
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
            assert!(request.explicit());
            assert!(request.release(request.snapshot()));
            assert!(!request.load(Ordering::Acquire));
            assert!(!request.explicit());
        }
    }

    #[test]
    fn automatic_request_has_its_own_generation_and_cannot_duplicate() {
        let request = ConnectionRequest::new(true);
        let old = request.snapshot();
        request.store(false, Ordering::Release);
        assert!(request.claim());
        assert!(!request.explicit());
        assert!(!request.claim());
        assert!(!request.release(old));
        assert!(request.load(Ordering::Acquire));
    }

    #[test]
    fn pairing_reservation_preserves_off_passive_and_explicit_audio_intent() {
        for flags in [0, 1, 3] {
            let request = ConnectionRequest(AtomicU64::new(flags));
            let old = request.snapshot();
            let renewed = request.renew();
            assert!(!request.is_current(old));
            assert!(request.is_current(renewed));
            assert_eq!(renewed & 3, flags);
            assert_eq!(request.load(Ordering::Acquire), flags & 1 != 0);
            assert_eq!(request.explicit(), flags == 3);
        }
    }

    #[test]
    fn passive_controls_cannot_page_after_the_windows_link_disappears() {
        let request = ConnectionRequest::new(false);
        assert!(request.claim());
        let passive = request.snapshot();
        assert!(request.may_open_controls(passive, true));
        assert!(!request.may_open_controls(passive, false));
        request.store(true, Ordering::Release);
        let explicit = request.snapshot();
        assert!(!request.may_open_controls(passive, true));
        assert!(request.may_open_controls(explicit, false));
        assert!(!request.release(passive));
        assert!(request.may_open_controls(explicit, false));
        assert!(request.release(explicit));
        assert!(!request.may_open_controls(explicit, true));
    }

    #[test]
    fn pairing_handoff_cannot_override_a_later_connection_command() {
        for connected in [false, true] {
            for next in [false, true] {
                let request = ConnectionRequest::new(connected);
                request.store(connected, Ordering::Release);
                let target = PairedTarget {
                    address: 1,
                    request: request.snapshot(),
                    stage: PairingStage::Validating,
                };
                assert!(target.current(&request));
                request.store(next, Ordering::Release);
                assert!(!target.current(&request));
                assert_eq!(request.load(Ordering::Acquire), next);
            }
        }
    }

    #[test]
    fn pending_pairing_blocks_automatic_selection_until_validated_or_superseded() {
        let connection = ConnectionRequest::new(false);
        let mut target = PairedTarget {
            address: 1,
            request: connection.snapshot(),
            stage: PairingStage::Validating,
        };
        assert!(!pairing_allows_selection(Some(&target), &connection, None));
        assert!(!pairing_allows_connect(Some(&target), &connection, 2));
        assert!(!pairing_allows_connect(Some(&target), &connection, 1));
        assert!(!pairing_allows_selection(
            Some(&target),
            &connection,
            Some(target.request)
        ));
        target.stage = PairingStage::Ready("New buds".into());
        assert!(!pairing_allows_connect(Some(&target), &connection, 2));
        assert!(!pairing_allows_connect(Some(&target), &connection, 1));
        assert!(!pairing_allows_selection(Some(&target), &connection, None));
        assert!(pairing_allows_selection(
            Some(&target),
            &connection,
            Some(target.request)
        ));
        target.stage = PairingStage::Selected;
        assert!(!pairing_allows_connect(Some(&target), &connection, 2));
        assert!(pairing_allows_connect(Some(&target), &connection, 1));
        assert!(!pairing_allows_selection(Some(&target), &connection, None));
        assert!(!pairing_allows_selection(
            Some(&target),
            &connection,
            Some(target.request)
        ));
        connection.store(false, Ordering::Release);
        assert!(pairing_allows_connect(Some(&target), &connection, 2));
        assert!(pairing_allows_selection(Some(&target), &connection, None));
        assert!(!pairing_allows_selection(
            Some(&target),
            &connection,
            Some(target.request)
        ));
        assert!(!pairing_allows_selection(
            None,
            &connection,
            Some(connection.snapshot())
        ));
    }
}
