use crate::runtime::{
    executor_pool::executor::Executor,
    reactors::Reactor,
    task::task::Task,
    utils::{backoff::LocalBackoff, cache_padded::CachePadded},
};
use lazy_static::lazy_static;
use std::{
    alloc::{Layout, alloc},
    cell::UnsafeCell,
    mem::MaybeUninit,
    num::NonZero,
    ops::Deref,
    ptr::{null_mut, slice_from_raw_parts_mut},
    sync::{
        Arc,
        atomic::{
            AtomicU64, AtomicUsize,
            Ordering::{AcqRel, Acquire, Release},
            fence,
        },
    },
    thread::{self, Thread},
};

const DEFAULT_NO_OF_EXE: usize = 8;
lazy_static! {
    pub(crate) static ref EXE_POOL: Wrapper = Wrapper::new();
}

#[repr(transparent)]
pub(crate) struct Wrapper {
    executor_pool: &'static ExecutorPool,
}
impl Wrapper {
    pub fn new() -> Self {
        let exe_pool = Wrapper {
            executor_pool: ExecutorPool::new(),
        };
        exe_pool.start();
        exe_pool
    }
}

impl Deref for Wrapper {
    type Target = ExecutorPool;
    fn deref(&self) -> &Self::Target {
        self.executor_pool
    }
}

fn get_no_of_cpus() -> usize {
    // Use the number of logical CPUs available
    let r = thread::available_parallelism()
        .unwrap_or(unsafe { NonZero::new_unchecked(DEFAULT_NO_OF_EXE) });
    r.get()
}

pub struct ExecutorPool {
    pub(crate) calibration_pending: CachePadded<AtomicUsize>,
    pub(crate) last_spawn_id: CachePadded<AtomicUsize>,
    pub(crate) active_task: CachePadded<AtomicUsize>,
    pub(crate) exe_mask: CachePadded<AtomicU64>,
    pub(crate) curr_exe: CachePadded<AtomicUsize>,
    pub(crate) n_executors: usize,
    pub(crate) reactor: &'static Reactor,
    pub(crate) executors: &'static mut [UnsafeCell<MaybeUninit<Executor>>],
    pub(crate) thread_handlers: &'static mut [UnsafeCell<MaybeUninit<Thread>>],
}

impl ExecutorPool {
    pub fn new() -> &'static Self {
        let reactor = Reactor::new();
        let n_executors = get_no_of_cpus().min(64);
        let calibration_pending = CachePadded::new(AtomicUsize::new(n_executors));
        let last_spawn_id = CachePadded::new(AtomicUsize::new(1));
        let curr_exe = CachePadded::new(AtomicUsize::new(0));
        let exe_mask = CachePadded::new(AtomicU64::new(u64::MAX >> (64 - n_executors)));
        let (exe_layout, th_layout) = Self::layout(n_executors);
        let active_task = CachePadded::new(AtomicUsize::new(0));
        let executors = unsafe {
            let ptr = alloc(exe_layout) as *mut UnsafeCell<MaybeUninit<Executor>>;
            if ptr != null_mut() {
                &mut *slice_from_raw_parts_mut(ptr, n_executors)
            } else {
                panic!("error while allocating memory")
            }
        };
        let thread_handlers = unsafe {
            let ptr = alloc(th_layout) as *mut UnsafeCell<MaybeUninit<Thread>>;
            if ptr != null_mut() {
                &mut *slice_from_raw_parts_mut(ptr, n_executors)
            } else {
                panic!("error while allocating memory")
            }
        };
        for idx in 0..n_executors {
            unsafe {
                thread_handlers
                    .as_mut_ptr()
                    .add(idx)
                    .write(UnsafeCell::new(MaybeUninit::uninit()));
                executors
                    .as_mut_ptr()
                    .add(idx)
                    .write(UnsafeCell::new(MaybeUninit::uninit()));
            }
        }
        let reactor = unsafe { &*Box::into_raw(Box::new(reactor)) };
        let exe_pool = unsafe {
            &*Box::into_raw(Box::new(Self {
                calibration_pending,
                last_spawn_id,
                active_task,
                exe_mask,
                curr_exe,
                reactor,
                n_executors,
                executors,
                thread_handlers,
            }))
        };
        for idx in 0..n_executors {
            let exe = Executor::new(idx, &exe_pool);
            let exe_deref = unsafe { &mut *exe_pool.executors[idx].get() };
            exe_deref.write(exe);
        }
        fence(Release);
        exe_pool
    }

    const fn layout(n: usize) -> (Layout, Layout) {
        let exe_layout = Layout::array::<UnsafeCell<MaybeUninit<Executor>>>(n);
        let th_layout = Layout::array::<UnsafeCell<MaybeUninit<Thread>>>(n);
        if let Ok(exe_layout) = exe_layout {
            if let Ok(th_layout) = th_layout {
                return (exe_layout, th_layout);
            }
        }
        panic!("error in layout");
    }

    pub fn start(&self) {
        fence(Acquire);
        for id in 0..self.n_executors {
            let exe = unsafe { (&*self.executors[id].get()).assume_init_ref() };
            thread::spawn(move || {
                let exe_pool = exe.exe_pool;
                unsafe { (&mut *exe_pool.thread_handlers[id].get()).write(thread::current()) };
                exe_pool.exe_mask.fetch_or(1 << id, Release);
                fence(Release);
                exe.run()
            });
        }
        let backoff = LocalBackoff::new();
        while self.calibration_pending.load(Acquire) != 0 {
            backoff.wait();
        }
    }

    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let boxed_fut = UnsafeCell::new(Box::pin(fut));
        let id = self.last_spawn_id.fetch_add(1, AcqRel);
        let task = Arc::new(Task::new(id, boxed_fut));
        self.push(task);
        println!("SPAWNED ID[{}]", id);
    }

    #[inline(always)]
    fn forward_idx(&self) -> usize {
        self.curr_exe
            .fetch_update(Release, Acquire, |prev| {
                let is_less = -((prev < self.n_executors) as isize);
                let next_idx = prev & is_less as usize;
                Some(next_idx)
            })
            .unwrap()
    }

    pub fn push(&self, task: Arc<Task>) {
        let exe_idx = self.forward_idx();
        let exe = unsafe { (&*self.executors[exe_idx].get()).assume_init_ref() };
        exe.push(task);

        let mask = 1 << exe_idx;
        let prev_mask = self.exe_mask.fetch_or(mask, AcqRel);
        if prev_mask & mask == 0 {
            // If we park the thread when it has no job here we need to unpark
            // self.thread_handlers
        }
    }
}
unsafe impl Sync for ExecutorPool {}
unsafe impl Send for ExecutorPool {}
