//! Sleep functionality using platform-native timers

use {
    crate::platform,
    pin_project::pin_project,
    std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll, Waker},
        time::Duration,
    },
};

/// A future that resolves after a specified duration
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

/// Sleep for the given duration
#[must_use]
pub fn sleep(duration: Duration) -> Sleep {
    Sleep::new(duration)
}

#[cfg(test)]
mod tests {
    use {super::*, std::time::Instant};

    #[test]
    fn test_sleep_basic() {
        let start = Instant::now();
        let duration = Duration::from_millis(50);

        blockon::block_on(sleep(duration));

        let elapsed = start.elapsed();
        assert!(elapsed >= duration);
        assert!(elapsed < duration + Duration::from_millis(50)); // Allow some tolerance
    }

    #[test]
    fn test_sleep_creation() {
        let duration = Duration::from_millis(100);
        let _sleep_future = sleep(duration);
        let _sleep_direct = Sleep::new(duration);

        // Should not panic during creation
    }

    #[test]
    fn test_sleep_cancel() {
        let mut sleep_future = Sleep::new(Duration::from_millis(1000));

        // Cancel the sleep immediately
        sleep_future.cancel();

        // The cancelled sleep should complete immediately
        let start = Instant::now();
        blockon::block_on(sleep_future);
        let elapsed = start.elapsed();

        // Should complete much faster than the original 1000ms
        assert!(elapsed < Duration::from_millis(100));
    }

    #[test]
    fn test_sleep_zero_duration() {
        let start = Instant::now();
        blockon::block_on(sleep(Duration::ZERO));
        let elapsed = start.elapsed();

        // Should complete very quickly
        assert!(elapsed < Duration::from_millis(10));
    }
}
