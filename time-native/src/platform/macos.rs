use {
    dispatch2::{DispatchQueue, DispatchTime},
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    },
};

#[derive(Debug)]
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
