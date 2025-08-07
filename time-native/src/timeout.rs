//! Timeout functionality using platform-native timers

use {
    crate::sleep::Sleep,
    pin_project::pin_project,
    std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll},
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sleep::sleep;
    use std::time::Instant;

    #[test]
    fn test_timeout_success() {
        let future = async {
            sleep(Duration::from_millis(30)).await;
            "completed"
        };
        
        let result = blockon::block_on(timeout(Duration::from_millis(100), future));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "completed");
    }

    #[test]
    fn test_timeout_elapsed() {
        let future = async {
            sleep(Duration::from_millis(100)).await;
            "completed"
        };
        
        let result = blockon::block_on(timeout(Duration::from_millis(30), future));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Elapsed);
    }

    #[test]
    fn test_timeout_immediate_completion() {
        let future = async { "immediate" };
        
        let result = blockon::block_on(timeout(Duration::from_millis(100), future));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "immediate");
    }

    #[test]
    fn test_timeout_zero_duration() {
        let future = async {
            sleep(Duration::from_millis(1)).await;
            "too_slow"
        };
        
        let result = blockon::block_on(timeout(Duration::ZERO, future));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Elapsed);
    }

    #[test]
    fn test_elapsed_error_display() {
        let elapsed = Elapsed;
        assert_eq!(format!("{}", elapsed), "operation timed out");
        assert_eq!(format!("{:?}", elapsed), "Elapsed");
    }

    #[test]
    fn test_elapsed_error_trait() {
        let elapsed = Elapsed;
        let error: &dyn std::error::Error = &elapsed;
        assert_eq!(error.to_string(), "operation timed out");
    }

    #[test] 
    fn test_timeout_timing_accuracy() {
        let start = Instant::now();
        let timeout_duration = Duration::from_millis(50);
        
        let future = async {
            sleep(Duration::from_millis(200)).await;
            "never_reached"
        };
        
        let result = blockon::block_on(timeout(timeout_duration, future));
        let elapsed = start.elapsed();
        
        assert!(result.is_err());
        // Should timeout close to the specified duration
        assert!(elapsed >= timeout_duration);
        assert!(elapsed < timeout_duration + Duration::from_millis(50));
    }
}
