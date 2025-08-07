//! Platform-native timeout implementation that doesn't depend on
//! runtime-specific reactors
//!
//! This provides timeout functionality using platform-native timers and
//! threading, making it work with any async runtime (Tokio, async-std, smol,
//! etc.)

use {
    pin_project::pin_project,
    std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll, Waker},
        time::Duration,
    },
};

/// Error type for timeout operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "operation timed out")
    }
}

impl std::error::Error for Elapsed {}

/// A future that resolves to an error if the given duration elapses
#[pin_project]
pub struct Sleep {
    timer: Arc<Mutex<TimerState>>,
    handle: Option<platform::TimerHandle>,
}

#[derive(Clone)]
enum TimerState {
    Pending { waker: Option<Waker> },
    Elapsed,
    Cancelled,
}

impl Sleep {
    /// Create a new sleep future that completes after the given duration
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        let timer = Arc::new(Mutex::new(TimerState::Pending { waker: None }));
        let timer_clone = timer.clone();

        let handle = platform::set_timer(duration, move || {
            let mut state = timer_clone.lock().unwrap();
            let TimerState::Pending { waker } = std::mem::replace(&mut *state, TimerState::Elapsed) else {
                // already completed or cancelled
                return;
            };

            if let Some(waker) = waker {
                waker.wake();
            }
        });

        Self { timer, handle: Some(handle) }
    }

    /// Cancel the timer
    pub fn cancel(&mut self) {
        if let Some(handle) = self.handle.take() {
            platform::cancel_timer(&handle);
            let mut state = self.timer.lock().unwrap();
            if let TimerState::Pending { waker: Some(waker) } = state.clone() {
                waker.wake_by_ref();
            }
            *state = TimerState::Cancelled;
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.timer.lock().unwrap();

        match &mut *state {
            TimerState::Cancelled | TimerState::Elapsed => Poll::Ready(()),
            TimerState::Pending { waker } => {
                if let Some(waker) = waker {
                    // Check if the stored waker matches the current tasks waker.
                    // This is necessary as the `Delay` future instance may move to
                    // a different task between calls to `poll`. If this happens, the
                    // waker contained by the given `Context` will differ and we
                    // must update our stored waker to reflect this change.
                    if !waker.will_wake(cx.waker()) {
                        waker.clone_from(cx.waker());
                    }
                } else {
                    *waker = Some(cx.waker().clone());
                }

                Poll::Pending
            }
        }
    }
}

/// A future that wraps another future with a timeout
#[pin_project]
pub struct Timeout<F> {
    #[pin]
    future: F,
    #[pin]
    sleep: Sleep,
    completed: Arc<AtomicBool>,
}

impl<F> Timeout<F> {
    fn new(future: F, duration: Duration) -> Self {
        Self {
            future,
            sleep: Sleep::new(duration),
            completed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<F> Future for Timeout<F>
where
    F: Future,
{
    type Output = Result<F::Output, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // If we've already completed, don't poll again
        if this.completed.load(Ordering::Acquire) {
            return Poll::Ready(Err(Elapsed));
        }

        // Try polling the main future first
        if let Poll::Ready(output) = this.future.poll(cx) {
            this.completed.store(true, Ordering::Release);
            this.sleep.cancel();
            return Poll::Ready(Ok(output));
        }

        // Check if timeout has elapsed
        if let Poll::Ready(()) = this.sleep.poll(cx) {
            this.completed.store(true, Ordering::Release);
            return Poll::Ready(Err(Elapsed));
        }

        Poll::Pending
    }
}

/// Wraps a future with a timeout
pub fn timeout<F>(duration: Duration, future: F) -> Timeout<F>
where
    F: Future,
{
    Timeout::new(future, duration)
}

/// Sleep for the given duration
#[must_use]
pub fn sleep(duration: Duration) -> Sleep {
    Sleep::new(duration)
}

// Platform-specific timer implementations
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "macos")]
mod macos {
    use {
        super::Duration,
        dispatch2::{DispatchQueue, DispatchTime},
        std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    pub struct TimerHandle {
        cancelled: Arc<AtomicBool>,
    }

    pub fn set_timer<F>(duration: Duration, callback: F) -> TimerHandle
    where
        F: FnOnce() + Send + 'static,
    {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = cancelled.clone();

        // Get global queue
        let queue = DispatchQueue::global_queue(dispatch2::GlobalQueueIdentifier::Priority(dispatch2::DispatchQueueGlobalPriority::Background));

        // Calculate when to execute (now + duration)
        let when = DispatchTime(DispatchTime::NOW.0 + DispatchTime::try_from(duration).unwrap_or(DispatchTime::NOW).0);

        // Schedule the block
        queue
            .after(when, move || {
                if !cancelled_clone.load(Ordering::Acquire) {
                    callback();
                }
            })
            .unwrap();

        TimerHandle { cancelled }
    }

    pub fn cancel_timer(handle: &TimerHandle) {
        handle.cancelled.store(true, Ordering::Release);
    }
}

#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(target_os = "windows")]
mod windows {
    use {
        super::Duration,
        std::{
            sync::{
                Arc,
                atomic::{AtomicBool, Ordering},
            },
            thread,
            time::Instant,
        },
        windows::{Win32::System::Threading::*, core::*},
    };

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

    pub fn cancel_timer(handle: TimerHandle) {
        handle.cancelled.store(true, Ordering::Release);
        // Timer thread will check this flag and exit
    }
}

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "linux")]
mod linux {
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

    pub fn cancel_timer(handle: TimerHandle) {
        handle.cancelled.store(true, Ordering::Release);
        // The thread will check this flag
    }
}

// Fallback implementation for unsupported platforms
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
use fallback as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod fallback {
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
            thread::sleep(duration);
            if !cancelled_clone.load(Ordering::Acquire) {
                callback();
            }
        });

        TimerHandle {
            cancelled,
            _thread_handle: thread_handle,
        }
    }

    pub fn cancel_timer(handle: TimerHandle) {
        handle.cancelled.store(true, Ordering::Release);
    }
}
