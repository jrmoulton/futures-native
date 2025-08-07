use {
    super::Duration,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    },
    windows::{Win32::System::Threading::*, core::*},
};

#[derive(Debug)]
pub struct TimerHandle {
    cancelled: Arc<AtomicBool>,
    _thread_handle: thread::JoinHandle<()>,
}

pub fn set_timer<F>(duration: Duration, callback: F) -> TimerHandle
where
    F: FnOnce() + Send + 'static,
{
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = cancelled.clone();

    let thread_handle = thread::spawn(move || {
        // Create a waitable timer
        let timer = unsafe { CreateWaitableTimerW(None, true, None).expect("Failed to create waitable timer") };

        // Convert duration to Windows FILETIME format (100ns intervals)
        let delay = -(duration.as_nanos() as i64 / 100);
        let due_time = delay as i64;

        unsafe {
            SetWaitableTimer(
                timer,
                &due_time as *const i64 as *const _,
                0,     // No period (one-shot)
                None,  // No completion routine
                None,  // No arg to completion routine
                false, // Don't resume system
            )
            .expect("Failed to set waitable timer");

            // Wait for the timer or cancellation
            let result = WaitForSingleObject(timer, INFINITE);
            CloseHandle(timer);

            // Only call callback if not cancelled
            if result == WAIT_OBJECT_0 && !cancelled_clone.load(Ordering::Acquire) {
                callback();
            }
        }
    });

    TimerHandle {
        cancelled,
        _thread_handle: thread_handle,
    }
}

pub fn cancel_timer(handle: &TimerHandle) {
    handle.cancelled.store(true, Ordering::Release);
    // Timer thread will check this flag and exit
}
