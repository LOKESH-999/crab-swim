use std::{
    cell::UnsafeCell,
    pin::Pin,
    sync::{
        Arc,
        atomic::{
            AtomicBool, AtomicU8,
            Ordering::{AcqRel, Acquire, Release},
        },
    },
    task::{Context, Poll},
};

// The extra 1 bit is reserved for future usecases
const TASK_MASK: u8 = 0b00001111;

const PROCESSING_FLAG: u8 = 0b01000000;
#[repr(u8)]
pub(crate) enum TaskState {
    // the task is in the queue and waiting for someone to pick and process it or in the process of being registered
    Registered = 0 << 0,
    // the task is being processed
    Processing = 1 << 0,
    // the task is completed
    Completed = 1 << 1,
    // the task is registered with waker the underlying resourse is not ready
    // and the task is waiting for it to be rea
    Waiting = 1 << 2,
}

pub(crate) struct Task {
    pub(crate) id: usize,
    pub(crate) fut: UnsafeCell<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    pub(crate) state: AtomicU8,
    #[cfg(panic = "unwind")]
    pub(crate) flag: AtomicBool,
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
            #[cfg(panic = "unwind")]
            flag: AtomicBool::new(false),
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    #[inline(always)]
    pub(crate) fn set_state(self: &Arc<Self>, state: TaskState) {
        // let val = (!TASK_MASK) | state as u8;
        let up_state = state as u8;
        let mut curr_state = self.state.load(Acquire);
        let mut new = (curr_state & (!TASK_MASK)) | up_state;
        while let Err(curr) = self
            .state
            .compare_exchange(curr_state, new, AcqRel, Acquire)
        {
            curr_state = curr;
            new = (curr & (!TASK_MASK)) | up_state
        }
    }

    #[inline(always)]
    pub(crate) fn set_processing_flag(&self) {
        self.state.fetch_or(PROCESSING_FLAG, Release);
    }

    #[inline(always)]
    pub(crate) fn unset_processing_flag(&self) {
        self.state.fetch_and(!PROCESSING_FLAG, Release);
    }
    #[inline(always)]
    pub(crate) fn get_state(self: &Arc<Self>) -> TaskState {
        unsafe { std::mem::transmute(self.state.load(Acquire) & TASK_MASK) }
    }

    #[inline(always)]
    pub(crate) fn cas(self: &Arc<Self>, current: TaskState, new: TaskState) -> Result<u8, u8> {
        self.state
            .compare_exchange(current as u8, new as u8, AcqRel, Acquire)
    }

    pub fn poll(self: &Arc<Self>, ctx: &mut Context<'_>) -> Poll<()> {
        let fut = unsafe { &mut *self.fut.get() };
        fut.as_mut().poll(ctx)
    }

    #[inline(always)]
    #[cfg(panic = "unwind")]
    fn is_pannicked(&self) -> bool {
        self.flag.load(Acquire)
    }
}
