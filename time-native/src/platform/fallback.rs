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

pub fn cancel_timer(handle: &TimerHandle) {
    handle.cancelled.store(true, Ordering::Release);
}
