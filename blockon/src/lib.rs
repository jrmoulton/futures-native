use std::{
    sync::{Arc, Condvar, Mutex},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

static VTABLE: RawWakerVTable = RawWakerVTable::new(
    // clone
    |data| {
        let arc = unsafe { Arc::from_raw(data as *const WakerData) };
        let cloned = arc.clone();
        std::mem::forget(arc);
        RawWaker::new(Arc::into_raw(cloned) as *const (), &VTABLE)
    },
    // wake
    |data| {
        let arc = unsafe { Arc::from_raw(data as *const WakerData) };
        let mut notified = arc.notified.lock().unwrap();
        *notified = true;
        arc.condvar.notify_one();
    },
    // wake_by_ref
    |data| {
        let arc = unsafe { &*(data as *const WakerData) };
        let mut notified = arc.notified.lock().unwrap();
        *notified = true;
        arc.condvar.notify_one();
    },
    // drop
    |data| {
        unsafe { Arc::from_raw(data as *const WakerData) };
    },
);

struct WakerData {
    condvar: Condvar,
    notified: Mutex<bool>,
}

pub fn block_on<F: Future<Output = O>, O>(fut: F) -> O {
    let waker_data = Arc::new(WakerData {
        condvar: Condvar::new(),
        notified: Mutex::new(false),
    });

    let raw_waker = RawWaker::new(Arc::into_raw(waker_data.clone()) as *const (), &VTABLE);
    let waker = unsafe { Waker::from_raw(raw_waker) };
    let mut context = Context::from_waker(&waker);

    let mut future = std::pin::pin!(fut);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(val) => break val,
            Poll::Pending => {
                let mut notified = waker_data.notified.lock().unwrap();
                if !*notified {
                    notified = waker_data.condvar.wait(notified).unwrap();
                }
                *notified = false;
            }
        }
    }
}

// impl ArcWake for WakerData {
//     fn wake_by_ref(arc_self: &Arc<Self>) {
//         let mut notified = arc_self.notified.lock().unwrap();
//         *notified = true;
//         arc_self.condvar.notify_one();
//     }
// }
