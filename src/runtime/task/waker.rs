use crate::runtime::{
    executor_pool::exe_pool::ExecutorPool,
    task::task::{
        Task,
        TaskState::{Registered, Waiting},
    },
};
use std::{
    sync::Arc,
    task::{RawWaker, RawWakerVTable},
};

// }
type WakerData = (Arc<Task>, &'static ExecutorPool);

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);

fn wake(data: *const ()) {
    wake_by_ref(data);
    let _ = unsafe { Box::from_raw(data as *mut WakerData) };
}

fn wake_by_ref(data: *const ()) {
    let waker_data = unsafe { &*(data as *mut WakerData) };
    let (task, exe_pool) = (waker_data.0.clone(), waker_data.1);
    if task.cas(Waiting, Registered).is_ok() {
        exe_pool.push(task);
    }
}

fn drop(data: *const ()) {
    _ = unsafe { Box::from_raw(data as *mut WakerData) }
}

fn clone(data: *const ()) -> RawWaker {
    let waker_data = unsafe { &*(data as *mut WakerData) };
    let (task, exe_pool) = (waker_data.0.clone(), waker_data.1);
    create_raw_waker((task, exe_pool))
}

pub(crate) fn create_raw_waker(waker_data: WakerData) -> RawWaker {
    let data = Box::into_raw(Box::new(waker_data)) as _;
    RawWaker::new(data, &VTABLE)
}
