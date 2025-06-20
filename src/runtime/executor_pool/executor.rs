use std::sync::Arc;

use crossbeam::queue::SegQueue;

use crate::runtime::{executor_pool::exe_pool::ExecutorPool, reactors::Reactor, task::task::Task};

pub struct Executor {
    queue: SegQueue<Arc<Task>>,
    reactor: &'static Reactor,
    exe_pool: &'static ExecutorPool,
}
