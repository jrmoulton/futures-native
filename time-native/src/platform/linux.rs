use {
    super::Duration,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    },
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
        // Use timerfd for high precision on Linux
        let timer_fd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };

        if timer_fd == -1 {
            // Fallback to simple sleep if timerfd fails
            thread::sleep(duration);
            if !cancelled_clone.load(Ordering::Acquire) {
                callback();
            }
            return;
        }

        let timer_spec = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            it_value: libc::timespec {
                tv_sec: duration.as_secs() as i64,
                tv_nsec: duration.subsec_nanos() as i64,
            },
        };

        unsafe {
            if libc::timerfd_settime(timer_fd, 0, &timer_spec, std::ptr::null_mut()) == -1 {
                libc::close(timer_fd);
                thread::sleep(duration);
                if !cancelled_clone.load(Ordering::Acquire) {
                    callback();
                }
                return;
            }

            // Wait for timer to expire
            let mut buffer = [0u8; 8];
            let result = libc::read(timer_fd, buffer.as_mut_ptr() as *mut libc::c_void, 8);

            libc::close(timer_fd);

            // Only call callback if read succeeded and not cancelled
            if result == 8 && !cancelled_clone.load(Ordering::Acquire) {
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
    // The thread will check this flag
}
