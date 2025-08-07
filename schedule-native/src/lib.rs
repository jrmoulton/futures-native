//! Platform-native task scheduling that leverages OS-specific APIs
//!
//! This crate provides a task scheduler that uses the most efficient
//! platform-native scheduling mechanisms: Grand Central Dispatch on macOS,
//! Windows Thread Pool API on Windows, and optimized thread pools on Linux.

pub use platform::PlatformScheduler as Scheduler;
use {
    async_task::Task,
    std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex, OnceLock},
        task::{Context, Poll, Waker},
        time::Duration,
    },
};

// Global scheduler instance to avoid recreating on every spawn
static GLOBAL_SCHEDULER: OnceLock<Scheduler> = OnceLock::new();

/// Priority levels for scheduled tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    /// Background priority - lowest
    Background,
    /// Utility priority - below normal
    Utility,
    /// Default priority - normal
    #[default]
    Default,
    /// User initiated priority - above normal
    UserInitiated,
    /// User interactive priority - highest
    UserInteractive,
}

/// Trait for platform-specific task schedulers
pub trait TaskScheduler: Clone + Send + Sync + 'static {
    /// Schedule a task to run immediately on background thread
    fn schedule<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static;

    /// Schedule a task to run with a specific priority on background thread
    fn schedule_with_priority<F>(&self, task: F, priority: Priority)
    where
        F: FnOnce() + Send + 'static;

    /// Schedule a task to run after a delay on background thread
    fn schedule_after<F>(&self, task: F, delay: Duration)
    where
        F: FnOnce() + Send + 'static;

    /// Schedule a task to run after a delay with priority on background thread
    fn schedule_after_with_priority<F>(&self, task: F, delay: Duration, priority: Priority)
    where
        F: FnOnce() + Send + 'static;

    /// Schedule a task to run immediately on main thread (if supported)
    /// Note: This can handle non-Send closures on main thread
    fn schedule_on_main_thread<F>(&self, task: F)
    where
        F: FnOnce() + 'static;

    /// Get the number of worker threads/queues
    fn worker_count(&self) -> usize;
}

/// A future that completes when scheduled on the platform scheduler
#[derive(Debug)]
pub struct Yield {
    scheduled: bool,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Yield {
    /// Create a new yield future
    pub fn new() -> Self {
        Self {
            scheduled: false,
            waker: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for Yield {
    fn default() -> Self {
        Self::new()
    }
}

impl Future for Yield {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.scheduled {
            return Poll::Ready(());
        }

        // Store the waker
        {
            let mut waker_guard = self.waker.lock().unwrap();
            *waker_guard = Some(cx.waker().clone());
        }

        // Schedule the wake-up
        let waker = self.waker.clone();
        GLOBAL_SCHEDULER.get_or_init(Scheduler::default).schedule(move || {
            if let Some(waker) = waker.lock().unwrap().take() {
                waker.wake();
            }
        });

        self.scheduled = true;
        Poll::Pending
    }
}

/// Yield execution to the scheduler
pub fn yield_now() -> Yield {
    Yield::new()
}

/// Spawn a task on the platform scheduler (background thread)
pub fn spawn<F>(future: F) -> Task<F::Output>
where
    F: Future<Output = ()> + Send + 'static,
{
    spawn_with_priority(future, Priority::Default)
}

/// Spawn a task on the main thread (if supported by platform)
pub fn spawn_on_main_thread<F>(future: F) -> Task<F::Output>
where
    F: Future + 'static,
{
    let scheduler = GLOBAL_SCHEDULER.get_or_init(Scheduler::default);

    // Use async-task to create a runnable that can handle non-Send futures
    let (runnable, task) = async_task::spawn_local(future, |runnable: async_task::Runnable| {
        scheduler.schedule_on_main_thread(move || {
            runnable.run();
        });
    });

    // Schedule the initial run
    runnable.schedule();

    task
}

/// Spawn a task with specific priority
pub fn spawn_with_priority<F>(future: F, priority: Priority) -> Task<F::Output>
where
    F: Future<Output = ()> + Send + 'static,
{
    let scheduler = GLOBAL_SCHEDULER.get_or_init(Scheduler::default);

    // Use async-task to create a runnable for background execution
    let (runnable, task) = async_task::spawn(future, move |runnable: async_task::Runnable| {
        scheduler.schedule_with_priority(
            move || {
                runnable.run();
            },
            priority,
        );
    });

    // Schedule the initial run
    runnable.schedule();

    task
}

/// Get the default platform scheduler
pub fn scheduler() -> Scheduler {
    GLOBAL_SCHEDULER.get_or_init(Scheduler::default).clone()
}

// Platform-specific implementations
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
use fallback as platform;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(target_os = "macos")]
mod macos {
    use {
        super::{Duration, Priority, TaskScheduler},
        dispatch2::{DispatchQueue, DispatchQueueGlobalPriority, DispatchTime, GlobalQueueIdentifier},
        std::sync::Arc,
    };

    /// macOS scheduler using Grand Central Dispatch
    #[derive(Clone, Debug)]
    pub struct PlatformScheduler {
        queues: Arc<QueueSet>,
    }

    #[derive(Debug)]
    struct QueueSet {
        background: dispatch2::DispatchRetained<DispatchQueue>,
        low: dispatch2::DispatchRetained<DispatchQueue>,
        default: dispatch2::DispatchRetained<DispatchQueue>,
        high: dispatch2::DispatchRetained<DispatchQueue>,
    }

    impl Default for PlatformScheduler {
        fn default() -> Self {
            let queues = QueueSet {
                background: DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Background)),
                low: DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Low)),
                default: DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::Default)),
                high: DispatchQueue::global_queue(GlobalQueueIdentifier::Priority(DispatchQueueGlobalPriority::High)),
            };

            Self { queues: Arc::new(queues) }
        }
    }

    impl PlatformScheduler {
        fn queue_for_priority(&self, priority: Priority) -> &dispatch2::DispatchRetained<DispatchQueue> {
            match priority {
                Priority::Background => &self.queues.background,
                Priority::Utility => &self.queues.low,
                Priority::Default => &self.queues.default,
                Priority::UserInitiated => &self.queues.high,
                Priority::UserInteractive => &self.queues.high,
            }
        }
    }

    impl TaskScheduler for PlatformScheduler {
        fn schedule<F>(&self, task: F)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule_with_priority(task, Priority::Default);
        }

        fn schedule_with_priority<F>(&self, task: F, priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            let queue = self.queue_for_priority(priority);
            // Schedule with minimal delay for immediate execution
            queue.exec_async(task);
        }

        fn schedule_after<F>(&self, task: F, delay: Duration)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule_after_with_priority(task, delay, Priority::Default);
        }

        fn schedule_after_with_priority<F>(&self, task: F, delay: Duration, priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            let queue = self.queue_for_priority(priority);
            let when = DispatchTime(DispatchTime::NOW.0 + DispatchTime::try_from(delay).unwrap_or(DispatchTime::NOW).0);
            queue.after(when, task).expect("Failed to schedule delayed task");
        }

        fn schedule_on_main_thread<F>(&self, task: F)
        where
            F: FnOnce() + 'static,
        {
            struct UnsafeSendWrapper<F>(F);

            unsafe impl<F> Send for UnsafeSendWrapper<F> {}

            impl<F> UnsafeSendWrapper<F> {
                fn call(self) -> F::Output
                where
                    F: FnOnce(),
                {
                    (self.0)()
                }
            }

            let wrapped_task = UnsafeSendWrapper(task);
            let main_queue = DispatchQueue::main();
            main_queue.exec_async(|| {
                wrapped_task.call();
            });
        }

        fn worker_count(&self) -> usize {
            // GCD manages this automatically, return a reasonable estimate
            std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4)
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use {
        super::{Duration, Priority, TaskScheduler},
        std::{ffi::c_void, ptr, sync::Arc, thread, time::Instant},
        windows::{Win32::System::Threading::*, core::*},
    };

    /// Windows scheduler using Thread Pool API
    #[derive(Clone, Debug)]
    pub struct PlatformScheduler {
        pool: Arc<ThreadPoolHandle>,
    }

    #[derive(Debug)]
    struct ThreadPoolHandle {
        pool: PTP_POOL,
        callback_environ: TP_CALLBACK_ENVIRON,
    }

    unsafe impl Send for ThreadPoolHandle {}
    unsafe impl Sync for ThreadPoolHandle {}

    impl Default for PlatformScheduler {
        fn default() -> Self {
            unsafe {
                let pool = CreateThreadpool(None).expect("Failed to create thread pool");

                let mut callback_environ = TP_CALLBACK_ENVIRON::default();
                InitializeThreadpoolEnvironment(&mut callback_environ);
                SetThreadpoolCallbackPool(&mut callback_environ, pool);

                Self {
                    pool: Arc::new(ThreadPoolHandle { pool, callback_environ }),
                }
            }
        }
    }

    impl Drop for ThreadPoolHandle {
        fn drop(&mut self) {
            unsafe {
                CloseThreadpool(self.pool);
                DestroyThreadpoolEnvironment(&mut self.callback_environ);
            }
        }
    }

    extern "system" fn work_callback(_instance: PTP_CALLBACK_INSTANCE, context: *mut c_void, _work: PTP_WORK) {
        let task = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce() + Send>) };
        task();
    }

    extern "system" fn timer_callback(_instance: PTP_CALLBACK_INSTANCE, context: *mut c_void, _timer: PTP_TIMER) {
        let task = unsafe { Box::from_raw(context as *mut Box<dyn FnOnce() + Send>) };
        task();
    }

    impl TaskScheduler for PlatformScheduler {
        fn schedule<F>(&self, task: F)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule_with_priority(task, Priority::Default);
        }

        fn schedule_with_priority<F>(&self, task: F, priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            unsafe {
                let task_box = Box::new(Box::new(task) as Box<dyn FnOnce() + Send>);
                let context = Box::into_raw(task_box) as *mut c_void;

                let work = CreateThreadpoolWork(Some(work_callback), Some(context), Some(&self.pool.callback_environ))
                    .expect("Failed to create threadpool work");

                // Set priority based on the enum
                let thread_priority = match priority {
                    Priority::Background => THREAD_PRIORITY_BELOW_NORMAL,
                    Priority::Utility => THREAD_PRIORITY_BELOW_NORMAL,
                    Priority::Default => THREAD_PRIORITY_NORMAL,
                    Priority::UserInitiated => THREAD_PRIORITY_ABOVE_NORMAL,
                    Priority::UserInteractive => THREAD_PRIORITY_HIGHEST,
                };

                SetThreadpoolCallbackPriority(&self.pool.callback_environ, thread_priority);
                SubmitThreadpoolWork(work);
                CloseThreadpoolWork(work);
            }
        }

        fn schedule_after<F>(&self, task: F, delay: Duration)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule_after_with_priority(task, delay, Priority::Default);
        }

        fn schedule_after_with_priority<F>(&self, task: F, delay: Duration, priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            unsafe {
                let task_box = Box::new(Box::new(task) as Box<dyn FnOnce() + Send>);
                let context = Box::into_raw(task_box) as *mut c_void;

                let timer = CreateThreadpoolTimer(Some(timer_callback), Some(context), Some(&self.pool.callback_environ))
                    .expect("Failed to create threadpool timer");

                // Convert duration to FILETIME format (100ns intervals)
                let delay_100ns = -(delay.as_nanos() as i64 / 100);
                let due_time = delay_100ns as i64;

                SetThreadpoolTimer(timer, Some(&due_time), 0, 0);
            }
        }

        fn schedule_on_main_thread<F>(&self, task: F)
        where
            F: FnOnce() + 'static,
        {
            // Windows doesn't have a direct main thread concept like macOS
            // Fall back to immediate execution on current thread
            task();
        }

        fn worker_count(&self) -> usize {
            std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4)
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use {
        super::{Duration, Priority, TaskScheduler},
        std::{
            sync::{
                Arc, Mutex,
                mpsc::{self, Receiver, Sender},
            },
            thread::{self, JoinHandle},
            time::Instant,
        },
    };

    /// Linux scheduler using optimized thread pool
    #[derive(Clone, Debug)]
    pub struct PlatformScheduler {
        sender: Sender<Task>,
        worker_count: usize,
    }

    type Task = Box<dyn FnOnce() + Send + 'static>;

    struct Worker {
        _handle: JoinHandle<()>,
    }

    impl Default for PlatformScheduler {
        fn default() -> Self {
            let worker_count = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4);

            let (sender, receiver) = mpsc::channel::<Task>();
            let receiver = Arc::new(Mutex::new(receiver));

            // Spawn worker threads
            let _workers: Vec<Worker> = (0..worker_count)
                .map(|_| {
                    let receiver = receiver.clone();
                    let handle = thread::spawn(move || {
                        loop {
                            let task = {
                                let receiver = receiver.lock().unwrap();
                                receiver.recv()
                            };

                            match task {
                                Ok(task) => task(),
                                Err(_) => break, // Channel closed
                            }
                        }
                    });

                    Worker { _handle: handle }
                })
                .collect();

            Self { sender, worker_count }
        }
    }

    impl TaskScheduler for PlatformScheduler {
        fn schedule<F>(&self, task: F)
        where
            F: FnOnce() + Send + 'static,
        {
            let _ = self.sender.send(Box::new(task));
        }

        fn schedule_with_priority<F>(&self, task: F, _priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            // Linux doesn't have built-in priority queues in the same way,
            // but we could implement multiple queues here if needed
            self.schedule(task);
        }

        fn schedule_after<F>(&self, task: F, delay: Duration)
        where
            F: FnOnce() + Send + 'static,
        {
            let sender = self.sender.clone();
            thread::spawn(move || {
                thread::sleep(delay);
                let _ = sender.send(Box::new(task));
            });
        }

        fn schedule_after_with_priority<F>(&self, task: F, delay: Duration, priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule_after(task, delay);
        }

        fn schedule_on_main_thread<F>(&self, task: F)
        where
            F: FnOnce() + 'static,
        {
            // Linux doesn't have a built-in main thread concept
            // Fall back to immediate execution on current thread
            task();
        }

        fn worker_count(&self) -> usize {
            self.worker_count
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod fallback {
    use {
        super::{Duration, Priority, TaskScheduler},
        std::{
            sync::{
                Arc, Mutex,
                mpsc::{self, Receiver, Sender},
            },
            thread::{self, JoinHandle},
        },
    };

    /// Fallback scheduler using basic thread pool
    #[derive(Clone, Debug)]
    pub struct PlatformScheduler {
        sender: Sender<Box<dyn FnOnce() + Send + 'static>>,
        worker_count: usize,
    }

    impl Default for PlatformScheduler {
        fn default() -> Self {
            let worker_count = std::thread::available_parallelism().map(|p| p.get()).unwrap_or(4);

            let (sender, receiver) = mpsc::channel();
            let receiver = Arc::new(Mutex::new(receiver));

            // Spawn worker threads
            for _ in 0..worker_count {
                let receiver = receiver.clone();
                thread::spawn(move || {
                    loop {
                        let task = {
                            let receiver = receiver.lock().unwrap();
                            receiver.recv()
                        };

                        match task {
                            Ok(task) => task(),
                            Err(_) => break,
                        }
                    }
                });
            }

            Self { sender, worker_count }
        }
    }

    impl TaskScheduler for PlatformScheduler {
        fn schedule<F>(&self, task: F)
        where
            F: FnOnce() + Send + 'static,
        {
            let _ = self.sender.send(Box::new(task));
        }

        fn schedule_with_priority<F>(&self, task: F, _priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule(task);
        }

        fn schedule_after<F>(&self, task: F, delay: Duration)
        where
            F: FnOnce() + Send + 'static,
        {
            let sender = self.sender.clone();
            thread::spawn(move || {
                thread::sleep(delay);
                let _ = sender.send(Box::new(task));
            });
        }

        fn schedule_after_with_priority<F>(&self, task: F, delay: Duration, _priority: Priority)
        where
            F: FnOnce() + Send + 'static,
        {
            self.schedule_after(task, delay);
        }

        fn schedule_on_main_thread<F>(&self, task: F)
        where
            F: FnOnce() + 'static,
        {
            // Fallback implementation: immediate execution on current thread
            task();
        }

        fn worker_count(&self) -> usize {
            self.worker_count
        }
    }
}
