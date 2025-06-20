use std::{
    cell::UnsafeCell,
    pin::Pin,
    sync::{
        Arc,
        atomic::{
            AtomicU8,
            Ordering::{AcqRel, Acquire, Release},
        },
    },
    task::{Context, Poll},
};

#[repr(u8)]
pub(crate) enum TaskState {
    // the task is in the queue and waiting for someone to pick and process it or in the process of being registered
    Registered = 0,
    // the task is being processed
    Processing = 1,
    // the task is completed
    Completed = 2,
    // the task is registered with waker the underlying resourse is not ready
    // and the task is waiting for it to be rea
    Waiting = 3,
}

pub(crate) struct Task {
    pub(crate) id: usize,
    pub(crate) fut: UnsafeCell<Pin<Box<dyn Future<Output = ()> + Send>>>,
    pub(crate) state: AtomicU8,
}

impl Task {
    pub(crate) fn new(
        id: usize,
        fut: UnsafeCell<Pin<Box<dyn Future<Output = ()> + Send>>>,
    ) -> Self {
        Self {
            id,
            fut,
            state: AtomicU8::new(TaskState::Registered as u8),
        }
    }

    pub(crate) fn set_state(self: &Arc<Self>, state: TaskState) {
        self.state.store(state as u8, Release);
    }

    pub(crate) fn get_state(self: &Arc<Self>) -> TaskState {
        unsafe { std::mem::transmute(self.state.load(Acquire)) }
    }

    pub fn cas(self: &Arc<Self>, current: TaskState, new: TaskState) -> Result<u8, u8> {
        self.state
            .compare_exchange(current as u8, new as u8, AcqRel, Acquire)
    }

    pub fn poll(self: &Arc<Self>, ctx: &mut Context<'_>) -> Poll<()> {
        let fut = unsafe { &mut *self.fut.get() };
        fut.as_mut().poll(ctx)
    }
}
