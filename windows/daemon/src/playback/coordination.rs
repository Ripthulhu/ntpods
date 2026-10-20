use super::protocol::invalid;
use crate::driver::Driver;
use std::io;

const VERSION: u32 = 1;
const QUERY: u32 = 0;
const ENABLE: u32 = 1;
const REPORT: u32 = 2;
const DISABLE: u32 = 3;
const ENABLED: u32 = 1;
const SUSPEND_REQUESTED: u32 = 2;
const REQUEST_PENDING: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub sequence: u32,
    pub suspend: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    generation: u32,
    sequence: u32,
    flags: u32,
}

impl State {
    fn pending(self) -> Option<Request> {
        (self.flags & REQUEST_PENDING != 0).then_some(Request {
            sequence: self.sequence,
            suspend: self.flags & SUSPEND_REQUESTED != 0,
        })
    }
}

pub(super) struct Coordinator {
    state: State,
    applied_policy_suspended: bool,
    producer_playing: bool,
}

impl Coordinator {
    pub fn enable(driver: &Driver) -> io::Result<Option<Self>> {
        Self::enable_with(|operation, generation| {
            exchange(driver, operation, generation, 0, false, false)
        })
    }

    fn enable_with(
        mut exchange: impl FnMut(u32, u32) -> io::Result<State>,
    ) -> io::Result<Option<Self>> {
        let queried = match exchange(QUERY, 0) {
            Ok(state) => state,
            Err(error) if unsupported(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        if queried.flags & ENABLED != 0 {
            return Err(invalid("Windows profile coordination is already owned"));
        }
        let state = match exchange(ENABLE, queried.generation) {
            Ok(state) => state,
            Err(error) if unsupported(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        require_active(state, queried.generation)?;
        Ok(Some(Self {
            state,
            applied_policy_suspended: false,
            producer_playing: false,
        }))
    }

    pub fn poll(&mut self, driver: &Driver) -> io::Result<()> {
        let state = exchange(driver, QUERY, 0, 0, false, false)?;
        require_active(state, self.state.generation)?;
        self.state = state;
        Ok(())
    }

    pub fn pending(&self) -> Option<Request> {
        self.state.pending()
    }

    pub fn suspended(&self) -> bool {
        self.applied_policy_suspended
    }

    pub fn allows_playback(&self) -> bool {
        !self.suspended() && !self.pending().is_some_and(|request| request.suspend)
    }

    pub fn report(
        &mut self,
        driver: &Driver,
        applied_policy_suspended: bool,
        producer_playing: bool,
        acknowledge: u32,
    ) -> io::Result<()> {
        if acknowledge == 0
            && self.applied_policy_suspended == applied_policy_suspended
            && self.producer_playing == producer_playing
        {
            return Ok(());
        }
        let state = exchange(
            driver,
            REPORT,
            self.state.generation,
            acknowledge,
            applied_policy_suspended,
            producer_playing,
        )?;
        self.accept_report(state, applied_policy_suspended, producer_playing)
    }

    fn accept_report(
        &mut self,
        state: State,
        applied_policy_suspended: bool,
        producer_playing: bool,
    ) -> io::Result<()> {
        require_active(state, self.state.generation)?;
        self.state = state;
        self.applied_policy_suspended = applied_policy_suspended;
        self.producer_playing = producer_playing;
        Ok(())
    }

    pub fn disable(&self, driver: &Driver) -> io::Result<()> {
        let state = exchange(driver, DISABLE, self.state.generation, 0, false, false)?;
        if state.generation != self.state.generation || state.flags & ENABLED != 0 {
            return Err(invalid("Windows profile coordination did not stop"));
        }
        Ok(())
    }
}

fn unsupported(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(1 | 50))
}

fn require_active(state: State, generation: u32) -> io::Result<()> {
    if state.generation != generation || state.flags & ENABLED == 0 {
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "Windows profile coordination changed with the playback connection",
        ))
    } else {
        Ok(())
    }
}

fn exchange(
    driver: &Driver,
    operation: u32,
    generation: u32,
    acknowledge: u32,
    suspended: bool,
    playing: bool,
) -> io::Result<State> {
    let input = request(operation, generation, acknowledge, suspended, playing);
    let mut output = [0; 16];
    let size = driver.playback_ioctl(0x815, &input, &mut output)?;
    parse(&output[..size])
}

fn request(
    operation: u32,
    generation: u32,
    acknowledge: u32,
    suspended: bool,
    playing: bool,
) -> [u8; 20] {
    let mut input = [0; 20];
    for (slot, value) in input.chunks_exact_mut(4).zip([
        VERSION,
        operation,
        generation,
        acknowledge,
        u32::from(suspended) | (u32::from(playing) << 1),
    ]) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
    input
}

fn parse(output: &[u8]) -> io::Result<State> {
    if output.len() != 16 {
        return Err(invalid("Truncated Windows profile coordination response"));
    }
    let word = |offset| u32::from_le_bytes(output[offset..offset + 4].try_into().unwrap());
    let state = State {
        generation: word(4),
        sequence: word(8),
        flags: word(12),
    };
    if word(0) != VERSION
        || state.flags & !(ENABLED | SUSPEND_REQUESTED | REQUEST_PENDING) != 0
        || (state.flags & ENABLED == 0 && state.flags != 0)
        || (state.flags & REQUEST_PENDING != 0 && state.sequence == 0)
    {
        return Err(invalid("Unsupported Windows profile coordination response"));
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(generation: u32, sequence: u32, flags: u32) -> Vec<u8> {
        [VERSION, generation, sequence, flags]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }

    #[test]
    fn wire_preserves_generation_wrap_and_separates_suspension_from_producer_intent() {
        assert_eq!(
            request(QUERY, 0, 0, false, false),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let input = request(REPORT, u32::MAX, 17, true, true);
        assert_eq!(&input[4..8], &REPORT.to_le_bytes());
        assert_eq!(&input[8..12], &u32::MAX.to_le_bytes());
        assert_eq!(&input[12..16], &17u32.to_le_bytes());
        assert_eq!(&input[16..], &3u32.to_le_bytes());
        assert_eq!(
            &request(REPORT, 0, 18, false, true)[16..],
            &2u32.to_le_bytes()
        );
        assert_eq!(
            &request(REPORT, 0, 19, true, false)[16..],
            &1u32.to_le_bytes()
        );
    }

    #[test]
    fn policy_requests_survive_same_value_repetitions_and_stale_acknowledgements() {
        let suspend = parse(&response(
            4,
            1,
            ENABLED | SUSPEND_REQUESTED | REQUEST_PENDING,
        ))
        .unwrap();
        assert_eq!(
            suspend.pending(),
            Some(Request {
                sequence: 1,
                suspend: true
            })
        );
        let newer = parse(&response(
            4,
            2,
            ENABLED | SUSPEND_REQUESTED | REQUEST_PENDING,
        ))
        .unwrap();
        assert_eq!(
            newer.pending(),
            Some(Request {
                sequence: 2,
                suspend: true
            })
        );
        let settled = parse(&response(4, 2, ENABLED | SUSPEND_REQUESTED)).unwrap();
        assert_eq!(settled.pending(), None);
        let resume = parse(&response(4, 3, ENABLED | REQUEST_PENDING)).unwrap();
        assert_eq!(
            resume.pending(),
            Some(Request {
                sequence: 3,
                suspend: false
            })
        );
    }

    #[test]
    fn malformed_or_replaced_driver_state_cannot_silently_resume_audio() {
        let valid = response(u32::MAX, 1, ENABLED | REQUEST_PENDING);
        for size in 0..16 {
            assert!(parse(&valid[..size]).is_err());
        }
        let mut oversized = valid.clone();
        oversized.push(0);
        assert!(parse(&oversized).is_err());
        for flags in [8, SUSPEND_REQUESTED, REQUEST_PENDING, u32::MAX] {
            assert!(parse(&response(1, 1, flags)).is_err());
        }
        assert!(parse(&response(1, 0, ENABLED | REQUEST_PENDING)).is_err());
        let mut unknown_version = valid.clone();
        unknown_version[0] = 2;
        assert!(parse(&unknown_version).is_err());
        assert!(require_active(parse(&valid).unwrap(), 0).is_err());
        assert!(require_active(parse(&response(0, 0, 0)).unwrap(), 0).is_err());
        assert!(require_active(parse(&response(0, 0, ENABLED)).unwrap(), 0).is_ok());
    }

    #[test]
    fn fallback_is_limited_to_an_unavailable_driver_contract() {
        for code in [1, 50] {
            assert!(unsupported(&io::Error::from_raw_os_error(code)));
        }
        for code in [5, 21, 87, 995, 1167] {
            assert!(!unsupported(&io::Error::from_raw_os_error(code)));
        }
    }

    #[test]
    fn a_new_request_during_suspend_is_preserved_without_losing_producer_intent() {
        let mut coordinator = Coordinator {
            state: parse(&response(
                4,
                1,
                ENABLED | SUSPEND_REQUESTED | REQUEST_PENDING,
            ))
            .unwrap(),
            applied_policy_suspended: false,
            producer_playing: true,
        };
        // Resume arrived while the old suspend operation was completing.
        let newer = parse(&response(4, 2, ENABLED | REQUEST_PENDING)).unwrap();
        coordinator.accept_report(newer, true, true).unwrap();
        assert!(coordinator.suspended());
        assert!(!coordinator.allows_playback());
        assert!(coordinator.producer_playing);
        assert_eq!(
            coordinator.pending(),
            Some(Request {
                sequence: 2,
                suspend: false
            })
        );
        assert!(coordinator
            .accept_report(
                State {
                    generation: 5,
                    ..newer
                },
                false,
                false
            )
            .is_err());
        assert!(coordinator.suspended());
        assert!(coordinator.producer_playing);
        coordinator
            .accept_report(parse(&response(4, 2, ENABLED)).unwrap(), false, false)
            .unwrap();
        assert!(!coordinator.suspended());
        assert!(coordinator.allows_playback());
        assert!(!coordinator.producer_playing);
        assert_eq!(coordinator.pending(), None);
    }

    #[test]
    fn a_pending_suspend_blocks_every_restart_before_it_is_acknowledged() {
        let mut coordinator = Coordinator {
            state: parse(&response(
                4,
                1,
                ENABLED | SUSPEND_REQUESTED | REQUEST_PENDING,
            ))
            .unwrap(),
            applied_policy_suspended: false,
            producer_playing: true,
        };
        assert!(!coordinator.suspended());
        assert!(!coordinator.allows_playback());
        coordinator
            .accept_report(
                parse(&response(4, 1, ENABLED | SUSPEND_REQUESTED)).unwrap(),
                true,
                true,
            )
            .unwrap();
        assert!(!coordinator.allows_playback());
        coordinator.state = parse(&response(4, 2, ENABLED | REQUEST_PENDING)).unwrap();
        assert!(!coordinator.allows_playback());
        coordinator
            .accept_report(parse(&response(4, 2, ENABLED)).unwrap(), false, false)
            .unwrap();
        assert!(coordinator.allows_playback());
        assert!(!coordinator.producer_playing);
    }

    #[test]
    fn an_older_driver_or_unsupported_image_never_acquires_coordination() {
        let mut calls = Vec::new();
        let result = Coordinator::enable_with(|operation, generation| {
            calls.push((operation, generation));
            Err(io::Error::from_raw_os_error(1))
        })
        .unwrap();
        assert!(result.is_none());
        assert_eq!(calls, [(QUERY, 0)]);

        for failure in [50, 5] {
            calls.clear();
            let result = Coordinator::enable_with(|operation, generation| {
                calls.push((operation, generation));
                if operation == QUERY {
                    Ok(State {
                        generation: 17,
                        sequence: 0,
                        flags: 0,
                    })
                } else {
                    Err(io::Error::from_raw_os_error(failure))
                }
            });
            assert_eq!(result.is_ok(), failure == 50);
            if failure == 50 {
                assert!(result.unwrap().is_none());
            }
            assert_eq!(calls, [(QUERY, 0), (ENABLE, 17)]);
        }
    }

    #[test]
    fn opt_in_cannot_attach_to_a_replacement_connection_or_existing_owner() {
        let mut calls = Vec::new();
        assert!(Coordinator::enable_with(|operation, generation| {
            calls.push((operation, generation));
            Ok(if operation == QUERY {
                State {
                    generation: 17,
                    sequence: 0,
                    flags: 0,
                }
            } else {
                State {
                    generation: 18,
                    sequence: 0,
                    flags: ENABLED,
                }
            })
        })
        .is_err());
        assert_eq!(calls, [(QUERY, 0), (ENABLE, 17)]);
        calls.clear();
        assert!(Coordinator::enable_with(|operation, generation| {
            calls.push((operation, generation));
            Ok(State {
                generation: 17,
                sequence: 0,
                flags: ENABLED,
            })
        })
        .is_err());
        assert_eq!(calls, [(QUERY, 0)]);
    }
}
