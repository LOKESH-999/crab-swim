use std::{
    num::NonZero,
    sync::atomic::{AtomicU64, AtomicUsize},
    thread::{self, Thread},
};

use crate::runtime::{executor_pool::executor::Executor, utils::cache_padded::CachePadded};

const DEFAULT_NO_OF_EXE: usize = 8;

fn get_no_of_cpus() -> usize {
    // Use the number of logical CPUs available
    let r = thread::available_parallelism()
        .unwrap_or(unsafe { NonZero::new_unchecked(DEFAULT_NO_OF_EXE) });
    r.get()
}

pub struct ExecutorPool {
    exe_mask: CachePadded<AtomicU64>,
    n_executors: usize,
    executors: &'static mut [Executor],
    thread_handlers: &'static mut [Thread],
    curr_exe: AtomicUsize,
}
