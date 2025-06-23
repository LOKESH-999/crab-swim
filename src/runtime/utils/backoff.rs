use std::{cell::Cell, hint::spin_loop};

/// A thread-local exponential backoff strategy for reducing contention.
///
/// `LocalBackoff` is useful in scenarios involving tight spin loops, such as in lock-free
/// data structures, where threads compete for the same resource. It exponentially increases
/// the number of CPU spin iterations each time `wait` is called, which helps reduce
/// contention and CPU usage under heavy load.

pub struct LocalBackoff {
    /// Tracks the current number of spin iterations for this thread.
    spins: Cell<u32>,
}

/// Maximum number of spin iterations allowed during backoff.
///
/// This is used to cap the exponential growth in spin iterations.
const MAX_SPIN: u32 = 1 << 16;

impl LocalBackoff {
    /// Creates a new `LocalBackoff` instance with an initial spin count of 0.
    ///
    /// Typically used in a local scope for retry-based synchronization primitives.
    pub fn new() -> Self {
        Self {
            spins: Cell::new(0),
        }
    }

    /// Performs a backoff by spinning the CPU in a tight loop.
    ///
    /// The number of iterations doubles with each call (up to `MAX_SPIN`), allowing
    /// contention to decrease before the next retry attempt. Internally, it uses
    /// [`std::hint::spin_loop`] to inform the CPU that it is in a spin-wait loop.
    pub fn wait(&self) {
        let curr_spin = self.spins.get();
        // Exponential backoff: increase spin count for next wait
        {
            let new_spin = (curr_spin << 1).min(MAX_SPIN);
            self.spins.set(new_spin);
        }

        // Perform the actual spin
        for _ in 0..curr_spin {
            spin_loop();
        }
    }

    /// Resets the backoff spin count to its initial value.
    ///
    /// This method is typically called after successful acquisition of a resource,
    /// such as a lock, to prepare the backoff mechanism for future contention scenarios.
    ///
    /// Resets the internal spin counter to `1`, which is the minimum non-zero spin.
    /// This avoids a completely zero-length spin on the next `wait()` and ensures
    /// a minimal delay is applied if immediate contention occurs again.
    pub fn reset(&self) {
        self.spins.set(1);
    }
}
