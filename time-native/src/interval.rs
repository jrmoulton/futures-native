//! Interval functionality using platform-native timers

use {
    crate::platform,
    futures_util::Stream,
    pin_project::pin_project,
    std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
        task::{Context, Poll, Waker},
        time::{Duration, Instant},
    },
};

/// Defines the behavior of an [`Interval`] when it misses a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissedTickBehavior {
    /// Ticks as fast as possible until caught up.
    /// This is the default behavior.
    Burst,
    /// Tick at multiples of `period` from when [`tick`] was called,
    /// rather than from the original start time.
    Delay,
    /// Skips missed ticks and tick on the next multiple of `period` from start.
    Skip,
}

impl Default for MissedTickBehavior {
    fn default() -> Self {
        Self::Burst
    }
}

impl MissedTickBehavior {
    /// If a tick is missed, this method determines when the next tick should
    /// happen.
    fn next_timeout(&self, timeout: Instant, now: Instant, period: Duration) -> Instant {
        match self {
            Self::Burst => timeout + period,
            Self::Delay => now + period,
            Self::Skip => {
                // Calculate how many periods have elapsed since the timeout
                let elapsed = now.saturating_duration_since(timeout);
                let period_nanos = period.as_nanos();

                if period_nanos == 0 {
                    return now + period;
                }

                let periods_elapsed = elapsed.as_nanos() / period_nanos;
                let next_period_count = periods_elapsed + 1;

                // Safely calculate the next timeout, avoiding overflow
                let additional_nanos = next_period_count.saturating_mul(period_nanos);
                if additional_nanos > u64::MAX as u128 {
                    // If we'd overflow, just use delay behavior as fallback
                    return now + period;
                }

                timeout + Duration::from_nanos(additional_nanos as u64)
            }
        }
    }
}

/// A stream of events that yields at fixed intervals
#[pin_project]
pub struct Interval {
    period: Duration,
    missed_tick_behavior: MissedTickBehavior,
    timer: Arc<Mutex<IntervalState>>,
}

#[derive(Debug)]
enum IntervalState {
    Pending {
        waker: Option<Waker>,
        handle: Option<platform::TimerHandle>,
        deadline: Instant,
    },
    Ready {
        deadline: Instant,
    },
}

impl Interval {
    /// Create a new interval that yields immediately and then at the given
    /// period.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new(period: Duration) -> Self {
        assert!(!period.is_zero(), "`period` must be non-zero");
        Self::new_at(Instant::now(), period)
    }

    /// Create a new interval that yields at `start` and then at the given
    /// period.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new_at(start: Instant, period: Duration) -> Self {
        assert!(!period.is_zero(), "`period` must be non-zero");

        Self {
            period,
            missed_tick_behavior: MissedTickBehavior::default(),
            timer: Arc::new(Mutex::new(IntervalState::Ready { deadline: start })),
        }
    }

    /// Completes when the next instant in the interval has been reached.
    pub async fn tick(&mut self) -> Instant {
        TickFuture::new(self).await
    }

    /// Polls for the next instant in the interval to be reached.
    pub fn poll_tick(&mut self, cx: &mut Context<'_>) -> Poll<Instant> {
        let mut state = self.timer.lock().unwrap();

        match &mut *state {
            IntervalState::Ready { deadline } => {
                let tick_instant = *deadline;
                let now = Instant::now();

                // Calculate next deadline
                let next_deadline = if now > tick_instant + Duration::from_millis(5) {
                    // We missed the tick - use the missed tick behavior
                    self.missed_tick_behavior.next_timeout(tick_instant, now, self.period)
                } else {
                    // Normal case - next tick is one period later
                    tick_instant + self.period
                };

                let delay = next_deadline.saturating_duration_since(now);

                if delay.is_zero() {
                    // Next tick is also ready immediately
                    *state = IntervalState::Ready { deadline: next_deadline };
                } else {
                    // Set up timer for the next tick
                    *state = IntervalState::Pending {
                        waker: Some(cx.waker().clone()),
                        handle: None,
                        deadline: next_deadline,
                    };

                    let timer_clone = self.timer.clone();
                    let handle = platform::set_timer(delay, move || {
                        let mut timer_state = timer_clone.lock().unwrap();

                        // We need to replace with a temporary value since IntervalState doesn't impl
                        // Default
                        let temp_deadline = Instant::now(); // This will be overwritten immediately
                        if let IntervalState::Pending { waker, deadline, .. } =
                            std::mem::replace(&mut *timer_state, IntervalState::Ready { deadline: temp_deadline })
                        {
                            // Clean up the handle
                            // if let Some(handle) = handle {
                            // Handle cleanup is platform-specific, but
                            // we're about to drop it anyway
                            // }

                            // Set the correct state with the original deadline
                            *timer_state = IntervalState::Ready { deadline };

                            if let Some(waker) = waker {
                                waker.wake();
                            }
                        }
                    });

                    // Update the handle in the state
                    if let IntervalState::Pending { handle: h, .. } = &mut *state {
                        *h = Some(handle);
                    }
                }

                Poll::Ready(tick_instant)
            }
            IntervalState::Pending { waker, .. } => {
                if let Some(existing_waker) = waker {
                    // Check if the stored waker matches the current task's waker.
                    // This is necessary as the `Interval` instance may move to
                    // a different task between calls to `poll`. If this happens, the
                    // waker contained by the given `Context` will differ and we
                    // must update our stored waker to reflect this change.
                    if !existing_waker.will_wake(cx.waker()) {
                        existing_waker.clone_from(cx.waker());
                    }
                } else {
                    *waker = Some(cx.waker().clone());
                }

                Poll::Pending
            }
        }
    }

    /// Reset the interval to complete one period after the current time.
    pub fn reset(&mut self) {
        self.reset_at(Instant::now() + self.period);
    }

    /// Reset the interval to complete immediately on the next tick.
    pub fn reset_immediately(&mut self) {
        self.reset_at(Instant::now());
    }

    /// Reset the interval to complete after the specified duration.
    pub fn reset_after(&mut self, after: Duration) {
        self.reset_at(Instant::now() + after);
    }

    /// Reset the interval to complete at the specified instant.
    pub fn reset_at(&mut self, deadline: Instant) {
        let mut state = self.timer.lock().unwrap();

        // Cancel any existing timer
        if let IntervalState::Pending { handle: Some(handle), .. } = &*state {
            platform::cancel_timer(handle);
        }

        *state = IntervalState::Ready { deadline };

        // If there was a waker waiting, wake it up since we changed the state
        // Note: We don't need to wake here since the next poll will see Ready
        // state
    }

    /// Returns the period of the interval.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Returns the current missed tick behavior.
    pub fn missed_tick_behavior(&self) -> MissedTickBehavior {
        self.missed_tick_behavior
    }

    /// Sets the missed tick behavior.
    pub fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
        self.missed_tick_behavior = behavior;
    }
}

impl Stream for Interval {
    type Item = Instant;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Use our existing poll_tick implementation
        match self.poll_tick(cx) {
            Poll::Ready(instant) => Poll::Ready(Some(instant)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A future returned by `Interval::tick`
#[pin_project]
struct TickFuture<'a> {
    interval: &'a mut Interval,
}

impl<'a> TickFuture<'a> {
    fn new(interval: &'a mut Interval) -> Self {
        Self { interval }
    }
}

impl<'a> Future for TickFuture<'a> {
    type Output = Instant;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.interval.poll_tick(cx)
    }
}

/// Create a new interval that yields immediately and then at the given period.
///
/// # Panics
///
/// Panics if `period` is zero.
#[must_use]
pub fn interval(period: Duration) -> Interval {
    Interval::new(period)
}

/// Create a new interval that yields at `start` and then at the given period.
///
/// # Panics
///
/// Panics if `period` is zero.
#[must_use]
pub fn interval_at(start: Instant, period: Duration) -> Interval {
    Interval::new_at(start, period)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[test]
    fn test_interval_basic() {
        let mut interval = interval(Duration::from_millis(30));
        let start = Instant::now();
        
        let first_tick = blockon::block_on(interval.tick());
        let first_elapsed = first_tick.duration_since(start);
        
        let second_tick = blockon::block_on(interval.tick());
        let second_elapsed = second_tick.duration_since(start);
        
        // First tick should be immediate
        assert!(first_elapsed < Duration::from_millis(10));
        
        // Second tick should be after the interval period
        assert!(second_elapsed >= Duration::from_millis(25));
        assert!(second_elapsed < Duration::from_millis(60));
    }

    #[test]
    fn test_interval_at() {
        let start = Instant::now();
        let future_time = start + Duration::from_millis(50);
        let mut interval = interval_at(future_time, Duration::from_millis(30));
        
        let first_tick = blockon::block_on(interval.tick());
        let elapsed = first_tick.duration_since(start);
        
        // First tick should happen at the specified time
        assert!(elapsed >= Duration::from_millis(40));
        assert!(elapsed < Duration::from_millis(80));
    }

    #[test]
    fn test_interval_stream() {
        let interval = interval(Duration::from_millis(20));
        
        let ticks = blockon::block_on(async {
            let mut ticks = Vec::new();
            let mut stream = interval.take(3);
            
            while let Some(instant) = stream.next().await {
                ticks.push(instant);
            }
            
            ticks
        });
        
        assert_eq!(ticks.len(), 3);
        
        // Check that intervals are roughly correct
        let interval_1 = ticks[1].duration_since(ticks[0]);
        let interval_2 = ticks[2].duration_since(ticks[1]);
        
        // Allow some tolerance for timing
        assert!(interval_1 >= Duration::from_millis(15));
        assert!(interval_1 < Duration::from_millis(40));
        assert!(interval_2 >= Duration::from_millis(15));
        assert!(interval_2 < Duration::from_millis(40));
    }

    #[test]
    fn test_interval_reset_behaviors() {
        let mut interval = interval(Duration::from_millis(100));
        
        // First tick is immediate
        let first = blockon::block_on(interval.tick());
        
        // Reset immediately and check that next tick is immediate again
        interval.reset_immediately();
        let reset_tick = blockon::block_on(interval.tick());
        
        // The reset tick should be very close to when we called reset
        let time_diff = reset_tick.duration_since(first);
        assert!(time_diff < Duration::from_millis(50));
        
        // Test reset_after
        let before_reset = Instant::now();
        interval.reset_after(Duration::from_millis(50));
        let after_reset_tick = blockon::block_on(interval.tick());
        let reset_elapsed = after_reset_tick.duration_since(before_reset);
        
        assert!(reset_elapsed >= Duration::from_millis(40));
        assert!(reset_elapsed < Duration::from_millis(80));
    }

    #[test]
    fn test_interval_missed_tick_behaviors() {
        // Test Burst behavior (default)
        let mut burst_interval = interval(Duration::from_millis(10));
        assert_eq!(burst_interval.missed_tick_behavior(), MissedTickBehavior::Burst);
        
        burst_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        assert_eq!(burst_interval.missed_tick_behavior(), MissedTickBehavior::Delay);
        
        burst_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        assert_eq!(burst_interval.missed_tick_behavior(), MissedTickBehavior::Skip);
    }

    #[test]
    fn test_interval_period() {
        let period = Duration::from_millis(42);
        let interval = interval(period);
        assert_eq!(interval.period(), period);
    }

    #[test]
    fn test_stream_with_combinators() {
        let result = blockon::block_on(async {
            let interval = interval(Duration::from_millis(15));
            
            let sum: usize = interval
                .take(5)
                .enumerate()
                .filter(|(i, _)| futures_util::future::ready(*i % 2 == 0))
                .map(|(i, _)| i)
                .fold(0, |acc, i| futures_util::future::ready(acc + i))
                .await;
            
            sum
        });
        
        // Should sum indices 0, 2, 4 = 6
        assert_eq!(result, 6);
    }

    #[test]
    #[should_panic(expected = "`period` must be non-zero")]
    fn test_interval_zero_period_panics() {
        let _ = interval(Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "`period` must be non-zero")]
    fn test_interval_at_zero_period_panics() {
        let _ = interval_at(Instant::now(), Duration::ZERO);
    }

    #[test]
    fn test_missed_tick_behavior_calculations() {
        let now = Instant::now();
        let timeout = now - Duration::from_millis(100);
        let period = Duration::from_millis(50);
        
        // Test Burst behavior
        let burst_next = MissedTickBehavior::Burst.next_timeout(timeout, now, period);
        assert_eq!(burst_next, timeout + period);
        
        // Test Delay behavior  
        let delay_next = MissedTickBehavior::Delay.next_timeout(timeout, now, period);
        assert_eq!(delay_next, now + period);
        
        // Test Skip behavior
        let skip_next = MissedTickBehavior::Skip.next_timeout(timeout, now, period);
        // Should skip ahead to next multiple
        assert!(skip_next > now);
        assert!(skip_next >= timeout + period * 2);
    }
}
