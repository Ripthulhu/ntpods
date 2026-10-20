use std::marker::PhantomData;
use std::rc::Rc;
use std::time::{Duration, Instant};
use windows::core::{Error, Result, HRESULT};
use windows::Win32::Foundation::{ERROR_CANCELLED, ERROR_NOT_FOUND, E_FAIL};
use windows::Win32::Media::Audio::{
    AudioCategory_Media, AudioClientProperties, IAudioClient3, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

pub(super) struct EnginePeriod {
    _client: IAudioClient3,
    // Release COM interfaces before leaving the apartment on this thread.
    _apartment: Apartment,
}

pub(super) struct Lease<T> {
    client: Option<T>,
    generation: Option<u32>,
}

impl<T> Default for Lease<T> {
    fn default() -> Self {
        Self {
            client: None,
            generation: None,
        }
    }
}

impl<T> Lease<T> {
    pub(super) fn update(
        &mut self,
        generation: Option<u32>,
        open: impl FnOnce() -> Option<T>,
    ) -> bool {
        if self.generation == generation {
            return false;
        }
        self.client.take();
        self.generation = generation;
        if generation.is_some() {
            self.client = open();
            return true;
        }
        false
    }

    pub(super) fn settled(&mut self, generation: Option<u32>) {
        // Initialization may itself replace the Windows producer.
        self.generation = generation;
        if generation.is_none() {
            self.client.take();
        }
    }
}

struct Apartment(PhantomData<Rc<()>>);

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Format(*mut WAVEFORMATEX);

impl Format {
    fn rate(&self) -> Result<u32> {
        unsafe { self.0.as_ref() }
            .map(|format| format.nSamplesPerSec)
            .filter(|rate| *rate != 0)
            .ok_or_else(|| Error::new(E_FAIL, "Windows returned no audio format"))
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

impl EnginePeriod {
    pub(super) fn acquire(address: u64, cancelled: impl FnMut() -> bool) -> Result<Self> {
        unsafe {
            let started = Instant::now();
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let apartment = Apartment(PhantomData);
            let endpoint = wait_for_endpoint(
                || crate::volume::native_output_device(address),
                cancelled,
                std::thread::sleep,
            )?;
            let endpoint_ms = started.elapsed().as_millis();
            let client: IAudioClient3 = endpoint.Activate(CLSCTX_ALL, None)?;
            let activated_ms = started.elapsed().as_millis();
            client.SetClientProperties(&AudioClientProperties {
                cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                eCategory: AudioCategory_Media,
                ..Default::default()
            })?;
            let format = Format(client.GetMixFormat()?);
            let rate = format.rate()?;
            let (mut normal, mut fundamental, mut minimum, mut maximum) = (0, 0, 0, 0);
            client.GetSharedModeEnginePeriod(
                format.0,
                &mut normal,
                &mut fundamental,
                &mut minimum,
                &mut maximum,
            )?;
            let (current_rate, current_frames) = current_period(&client)?;
            let frames = requested_frames(
                rate,
                current_rate,
                current_frames,
                fundamental,
                minimum,
                maximum,
            )
            .ok_or_else(|| Error::new(E_FAIL, "A compatible 5 ms audio period is unavailable"))?;
            let queried_ms = started.elapsed().as_millis();
            // Starting this client would keep Bluetooth playback active when music is paused.
            client.InitializeSharedAudioStream(0, frames, format.0, None)?;
            let (actual_rate, actual_frames) = current_period(&client)?;
            if !verified_period(rate, frames, actual_rate, actual_frames) {
                return Err(Error::new(
                    E_FAIL,
                    "Windows did not select the requested audio period",
                ));
            }
            crate::log(&format!(
                "playback: Windows shared period={actual_frames} frames at {actual_rate} Hz (unstarted client), endpoint_ms={endpoint_ms} activate_ms={} query_ms={} initialize_ms={}",
                activated_ms.saturating_sub(endpoint_ms),
                queried_ms.saturating_sub(activated_ms),
                started.elapsed().as_millis().saturating_sub(queried_ms),
            ));
            Ok(Self {
                _client: client,
                _apartment: apartment,
            })
        }
    }
}

fn current_period(client: &IAudioClient3) -> Result<(u32, u32)> {
    let mut format = Format(std::ptr::null_mut());
    let mut frames = 0;
    unsafe { client.GetCurrentSharedModeEnginePeriod(&mut format.0, &mut frames)? };
    Ok((format.rate()?, frames))
}

fn requested_frames(
    rate: u32,
    current_rate: u32,
    current: u32,
    fundamental: u32,
    minimum: u32,
    maximum: u32,
) -> Option<u32> {
    let scaled = u64::from(rate) * 5;
    if rate == 0 || rate != current_rate || current == 0 || scaled % 1000 != 0 {
        return None;
    }
    let frames = (scaled / 1000) as u32;
    let frames = frames.min(current);
    (frames != 0
        && fundamental != 0
        && minimum <= maximum
        && frames >= minimum
        && frames <= maximum
        && frames % fundamental == 0)
        .then_some(frames)
}

fn verified_period(rate: u32, requested: u32, actual_rate: u32, actual: u32) -> bool {
    rate != 0
        && actual_rate == rate
        && actual != 0
        && actual <= requested
        && u64::from(actual) * 1000 <= u64::from(rate) * 5
}

fn wait_for_endpoint<T>(
    mut find: impl FnMut() -> Result<T>,
    mut cancelled: impl FnMut() -> bool,
    mut wait: impl FnMut(Duration),
) -> Result<T> {
    for attempt in 0..5 {
        if cancelled() {
            return Err(Error::from_hresult(HRESULT::from_win32(ERROR_CANCELLED.0)));
        }
        match find() {
            Ok(endpoint) => {
                if cancelled() {
                    return Err(Error::from_hresult(HRESULT::from_win32(ERROR_CANCELLED.0)));
                }
                return Ok(endpoint);
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) && attempt < 4 => {
                wait(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn request_respects_range_alignment_and_faster_existing_periods() {
        assert_eq!(
            requested_frames(48_000, 48_000, 480, 48, 240, 480),
            Some(240)
        );
        assert_eq!(
            requested_frames(48_000, 48_000, 192, 48, 192, 480),
            Some(192)
        );
        assert_eq!(
            requested_frames(96_000, 96_000, 960, 96, 384, 960),
            Some(480)
        );
        for (rate, current_rate, current, fundamental, minimum, maximum) in [
            (0, 0, 480, 48, 240, 480),
            (44_100, 44_100, 441, 1, 220, 441),
            (48_000, 96_000, 480, 48, 240, 480),
            (48_000, 48_000, 0, 48, 240, 480),
            (48_000, 48_000, 480, 0, 240, 480),
            (48_000, 48_000, 480, 48, 480, 240),
            (48_000, 48_000, 480, 48, 480, 480),
            (48_000, 48_000, 480, 48, 48, 192),
            (48_000, 48_000, 480, 96, 192, 480),
            (48_000, 48_000, 192, 48, 240, 480),
        ] {
            assert_eq!(
                requested_frames(rate, current_rate, current, fundamental, minimum, maximum),
                None
            );
        }
    }

    #[test]
    fn verification_requires_the_selected_rate_and_no_slower_period() {
        assert!(verified_period(48_000, 240, 48_000, 240));
        assert!(verified_period(48_000, 240, 48_000, 192));
        assert!(!verified_period(48_000, 240, 44_100, 240));
        assert!(!verified_period(48_000, 240, 48_000, 480));
        assert!(!verified_period(48_000, 192, 48_000, 240));
        assert!(!verified_period(48_000, 480, 48_000, 480));
        assert!(!verified_period(48_000, 240, 48_000, 0));
    }

    #[test]
    fn retries_only_missing_endpoints_and_stops_after_four_waits() {
        for (code, expected_calls) in [
            (HRESULT::from_win32(ERROR_NOT_FOUND.0), 5),
            (HRESULT::from_win32(ERROR_CANCELLED.0), 1),
            (E_FAIL, 1),
        ] {
            let calls = Cell::new(0);
            let waits = Cell::new(0);
            let result = wait_for_endpoint::<()>(
                || {
                    calls.set(calls.get() + 1);
                    Err(Error::from_hresult(code))
                },
                || false,
                |delay| {
                    assert_eq!(delay, Duration::from_millis(100));
                    waits.set(waits.get() + 1);
                },
            );
            assert_eq!(result.unwrap_err().code(), code);
            assert_eq!(calls.get(), expected_calls);
            assert_eq!(waits.get(), expected_calls - 1);
        }
    }

    #[test]
    fn a_late_endpoint_is_kept_after_one_successful_retry() {
        let calls = Cell::new(0);
        let result = wait_for_endpoint(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Err(Error::from_hresult(HRESULT::from_win32(ERROR_NOT_FOUND.0)))
                } else {
                    Ok(7)
                }
            },
            || false,
            |_| {},
        );
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn cancellation_during_notification_wait_prevents_another_lookup() {
        let cancelled = Cell::new(false);
        let calls = Cell::new(0);
        let result = wait_for_endpoint::<()>(
            || {
                calls.set(calls.get() + 1);
                Err(Error::from_hresult(HRESULT::from_win32(ERROR_NOT_FOUND.0)))
            },
            || cancelled.get(),
            |_| cancelled.set(true),
        );
        assert_eq!(
            result.unwrap_err().code(),
            HRESULT::from_win32(ERROR_CANCELLED.0)
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn cancellation_releases_a_just_resolved_endpoint() {
        let cancelled = Cell::new(false);
        let endpoint = Rc::new(());
        let result = wait_for_endpoint(
            || {
                cancelled.set(true);
                Ok(endpoint.clone())
            },
            || cancelled.get(),
            |_| panic!("resolved endpoints do not retry"),
        );
        assert_eq!(
            result.unwrap_err().code(),
            HRESULT::from_win32(ERROR_CANCELLED.0)
        );
        assert_eq!(Rc::strong_count(&endpoint), 1);
    }

    #[test]
    fn lease_exists_only_while_a_real_producer_runs() {
        let endpoint = Rc::new(());
        let mut lease = Lease::default();
        assert!(!lease.update(None, || panic!("idle must not open an audio client")));
        assert!(lease.update(Some(1), || Some(endpoint.clone())));
        assert_eq!(Rc::strong_count(&endpoint), 2);
        assert!(!lease.update(Some(1), || panic!("running client must be retained")));
        assert!(!lease.update(None, || panic!("idle must only release the client")));
        assert_eq!(Rc::strong_count(&endpoint), 1);
        assert!(lease.update(Some(2), || Some(endpoint.clone())));
        drop(lease);
        assert_eq!(Rc::strong_count(&endpoint), 1);
    }

    #[test]
    fn failed_lease_is_retried_only_after_the_producer_changes() {
        let mut lease = Lease::<()>::default();
        assert!(lease.update(Some(1), || None));
        assert!(!lease.update(Some(1), || panic!("failure must not retry while running")));
        assert!(lease.update(Some(2), || None));
        assert!(!lease.update(None, || panic!("idle must not retry")));
        assert!(lease.update(Some(3), || Some(())));
    }

    #[test]
    fn missed_stop_start_releases_the_old_client_before_reacquiring() {
        let endpoint = Rc::new(());
        let mut lease = Lease::default();
        assert!(lease.update(Some(u32::MAX), || Some(endpoint.clone())));
        assert_eq!(Rc::strong_count(&endpoint), 2);
        assert!(lease.update(Some(0), || {
            assert_eq!(Rc::strong_count(&endpoint), 1);
            Some(endpoint.clone())
        }));
        assert_eq!(Rc::strong_count(&endpoint), 2);
        assert!(!lease.update(Some(0), || panic!("same producer must not retry")));
    }

    #[test]
    fn initialization_adopts_its_own_producer_change_without_retrying() {
        let endpoint = Rc::new(());
        let mut lease = Lease::default();
        assert!(lease.update(Some(1), || Some(endpoint.clone())));
        lease.settled(Some(3));
        assert!(!lease.update(Some(3), || panic!("initialization must not trigger itself")));
        assert_eq!(Rc::strong_count(&endpoint), 2);
        lease.settled(None);
        assert_eq!(Rc::strong_count(&endpoint), 1);
        assert!(lease.update(Some(4), || Some(endpoint.clone())));
    }
}
