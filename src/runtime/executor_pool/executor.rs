use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    task::{
        Context,
        Poll::{Pending, Ready},
        Waker,
    },
};

use crate::runtime::{
    executor_pool::exe_pool::ExecutorPool,
    task::{
        task::{
            Task,
            TaskState::{Completed, Processing, Registered, Waiting},
        },
        waker,
    },
    utils::backoff::LocalBackoff,
};
use crossbeam::queue::SegQueue;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};

pub struct Executor {
    pub(crate) id: usize,
    pub(crate) queue: SegQueue<Arc<Task>>,
    pub(crate) exe_pool: &'static ExecutorPool,
}

impl Executor {
    pub fn new(id: usize, exe_pool: &'static ExecutorPool) -> Self {
        let queue = SegQueue::new();
        Self {
            id,
            queue,
            exe_pool,
        }
    }
    pub fn run(&self) -> ! {
        let backoff = LocalBackoff::new();
        self.exe_pool.calibration_pending.fetch_sub(1, AcqRel);
        while self.exe_pool.calibration_pending.load(Acquire) != 0 {
            backoff.wait();
        }

        loop {
            let task = self.queue.pop();
            match task {
                Some(t) => {
                    println!("TASK ID:{}", t.id);
                    if t.cas(Registered, Processing).is_ok() {
                        self.process_task(t);
                    }
                }
                None => {
                    let mut exe_mask = self.exe_pool.exe_mask.load(Acquire);
                    let mut new = exe_mask & !(1 << self.id);
                    // we can also do fetch_and
                    while let Err(curr) = self
                        .exe_pool
                        .exe_mask
                        .compare_exchange(exe_mask, new, AcqRel, Acquire)
                    {
                        exe_mask = curr;
                        new = curr & !(1 << self.id);
                    }
                    let nereast_exe_tast = self.exe_pool.exe_mask.load(Acquire).trailing_zeros();
                    if nereast_exe_tast < self.exe_pool.n_executors as u32 {
                        let task = unsafe {
                            (&*self.exe_pool.executors[nereast_exe_tast as usize].get())
                                .assume_init_ref()
                                .queue
                                .pop()
                        };
                        if let Some(task) = task {
                            if task.cas(Registered, Processing).is_ok() {
                                println!("TASK_ID:{}", task.id);
                                self.process_task(task);
                            }
                        }
                    }
                    // todo!("work stealing")
                }
            }
        }
    }

    #[inline(always)]
    fn process_task(&self, task: Arc<Task>) {
        let waker_data = (task.clone(), self.exe_pool);
        let raw_waker = waker::create_raw_waker(waker_data);
        let waker = unsafe { Waker::from_raw(raw_waker) };
        let mut ctx = Context::from_waker(&waker);
        // Setting Processing flag in the Task State so that we can avoid double polling
        task.set_processing_flag();
        #[cfg(not(panic = "unwind"))]
        {
            match task.poll(&mut ctx) {
                Ready(_) => {
                    task.set_state(Completed);
                    // TODO Join Handler
                }
                Pending => {
                    // We unset the processing flag So that the CAS works
                    task.unset_processing_flag();
                    if task.cas(Processing, Registered).is_ok() {
                        self.exe_pool.push(task);
                    }
                }
            }
        }
        #[cfg(panic = "unwind")]
        {
            let poll_result = catch_unwind(AssertUnwindSafe(|| task.poll(&mut ctx)));
            match poll_result {
                Ok(Ready(_)) => {
                    task.set_state(Completed);
                    // TODO Join Handler
                }
                Ok(Pending) => {
                    // We unset the processing flag So that the CAS works
                    task.unset_processing_flag();
                    if task.cas(Processing, Registered).is_ok() {
                        self.exe_pool.push(task);
                    }
                }
                Err(err) => {
                    task.set_state(Completed);
                    task.flag.store(true, Release);
                    eprintln!("task: id: [{}], Err:[{:?}]", task.id(), err);
                }
            }
        }
    }

    #[inline(always)]
    pub fn push(&self, task: Arc<Task>) {
        self.queue.push(task);
    }
}
unsafe impl Send for Executor {}
unsafe impl Sync for Executor {}
