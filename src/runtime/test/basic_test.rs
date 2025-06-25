use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::Duration;

use crate::runtime::{
    executor_pool::exe_pool::{EXE_POOL, ExecutorPool},
    task::yield_now,
};

#[test]
fn test_basic_task_execution() {
    const N: usize = 10;
    let counter = Arc::new(AtomicUsize::new(0));
    let exe = ExecutorPool::new();
    exe.start();
    for i in 0..N {
        println!("task{i}");
        let counter = counter.clone();
        exe.spawn(async move {
            // simulate lightweight async task
            counter.fetch_add(1, Ordering::AcqRel);
            yield_now().await
        });
    }

    // Give some time for the executors to run
    thread::sleep(Duration::from_millis(3000));

    let val = counter.load(Ordering::Acquire);
    assert_eq!(val, N, "Expected all {} tasks to complete", N);
}

#[test]
fn stress_test_yield_and_atomic_deterministic() {
    const NUM_TASKS: usize = 1100000;
    const INCREMENTS_PER_TASK: usize = 1000;
    const EXPECTED: usize = NUM_TASKS * INCREMENTS_PER_TASK;

    let counter = Arc::new(AtomicUsize::new(0));

    for task_id in 0..NUM_TASKS {
        let counter = counter.clone();
        EXE_POOL.spawn(async move {
            for i in 0..INCREMENTS_PER_TASK {
                counter.fetch_add(1, Ordering::AcqRel);

                // Deterministic yield pattern:
                // Yield every 10th and every 250th iteration
                if i % 10 == 0 || i % 250 == 0 {
                    yield_now().await;
                    f(i % 10 == 0).await;
                }
            }
        });
    }

    // Wait for tasks to complete
    thread::sleep(Duration::from_secs(60));

    let final_count = counter.load(Ordering::Acquire);
    assert_eq!(
        final_count, EXPECTED,
        "Expected counter to be {}, but got {}",
        EXPECTED, final_count
    );
}

async fn f(x: bool) -> Arc<u32> {
    if x { Arc::new(1) } else { Arc::new(2) }
}
