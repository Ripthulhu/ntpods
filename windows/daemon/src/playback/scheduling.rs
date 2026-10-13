use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;

#[link(name = "avrt")]
extern "system" {
    fn AvSetMmThreadCharacteristicsW(task: *const u16, index: *mut u32) -> *mut c_void;
    fn AvRevertMmThreadCharacteristics(task: *mut c_void) -> i32;
}

pub(super) struct AudioScheduling {
    task: *mut c_void,
    // MMCSS registration must be reverted on the thread that acquired it.
    _thread: PhantomData<Rc<()>>,
}

impl AudioScheduling {
    pub(super) fn acquire() -> Option<Self> {
        Self::register("Audio")
    }

    fn register(name: &str) -> Option<Self> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut index = 0;
        let task = unsafe { AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut index) };
        (!task.is_null()).then(|| Self {
            task,
            _thread: PhantomData,
        })
    }
}

impl Drop for AudioScheduling {
    fn drop(&mut self) {
        unsafe {
            AvRevertMmThreadCharacteristics(self.task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_task_leaves_scheduling_unchanged() {
        assert!(AudioScheduling::register("NTPods nonexistent MMCSS test task").is_none());
    }

    #[test]
    #[ignore = "requires the Windows MMCSS service"]
    fn audio_registration_is_reverted_on_drop_and_unwind() {
        let guard = AudioScheduling::acquire().expect("Audio MMCSS registration failed");
        let task = guard.task;
        drop(guard);
        assert_eq!(unsafe { AvRevertMmThreadCharacteristics(task) }, 0);

        let mut task = std::ptr::null_mut();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let guard = AudioScheduling::acquire().expect("Audio MMCSS registration failed");
            task = guard.task;
            panic!("test worker failure");
        }));
        assert!(result.is_err());
        assert!(!task.is_null());
        assert_eq!(unsafe { AvRevertMmThreadCharacteristics(task) }, 0);
    }
}
