use std::{sync::Arc, thread};

use crate::runtime::{
    executor_pool::exe_pool::ExecutorPool, reactors::Reactor, task::task::{Task,TaskState::{Completed,Registered,Processing,Waiting}},
    utils::backoff::LocalBackoff,
};
use crossbeam::queue::SegQueue;
use std::sync::atomic::Ordering::{AcqRel, Acquire};

pub struct Executor {
    pub(crate) id: usize,
    pub(crate) queue: SegQueue<Arc<Task>>,
    pub(crate) reactor: &'static Reactor,
    pub(crate) exe_pool: &'static ExecutorPool,
}

impl Executor {
    pub fn new(id: usize, exe_pool: &'static ExecutorPool, reactor: &'static Reactor) -> Self {
        let queue = SegQueue::new();
        Self {
            id,
            queue,
            reactor,
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
                    if t.cas(Registered, Processing).is_ok(){
                        self.process_task(t);
                    }
                }
                None => {
                    let mut exe_mask = self.exe_pool.exe_mask.load(Acquire);
                    let mut new = exe_mask & !(1 << self.id);
                    while let Err(curr) = self
                        .exe_pool
                        .exe_mask
                        .compare_exchange(exe_mask, new, AcqRel, Acquire)
                    {
                        exe_mask = curr;
                        new = curr & !(1 << self.id);
                    }
                    thread::park();
                    todo!("work stealing")
                }
            }
        }
    }

    #[inline(always)]
    fn process_task(&self,task:Arc<Task>){
        todo!()
    }

    #[inline(always)]
    pub fn push(&self, data: Arc<Task>) {
        self.queue.push(data);
    }
}
unsafe impl Send for Executor {}
unsafe impl Sync for Executor {}
