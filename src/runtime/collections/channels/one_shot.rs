use std::{
    cell::UnsafeCell,
    future::poll_fn,
    mem::MaybeUninit,
    pin::Pin,
    sync::atomic::{
        AtomicU8,
        Ordering::{AcqRel, Acquire, Release},
        fence,
    },
    task::{
        Context,
        Poll::{self, Pending, Ready},
        Waker,
    },
};

use crate::runtime::task::{task::TaskState, waker::WakerData};

/// Bitmask to indicate that either `Sender` or `Receiver` has been dropped.
///
/// Used in dealocating the underlying OneShot Data
const DROP_INDICATOR_MASK: u8 = 0b00010000;

/// Bitmask to indicate that the `Sender`'s waker has been registered.
///
/// Also serves to know if it needs to be dropped.
const TX_WAKER_REG_MASK: u8 = 0b01000000;

/// Bitmask to indicate that the `Receiver`'s waker has been registered.
///
/// Also used to trigger a wake-up when the slot is filled.
const RX_WAKER_REG_MASK: u8 = 0b00100000;

/// Bitmask to extract the current state of the slot (e.g., Empty or Equipped).
const STATE_READ_MASK: u8 = 0b00000011;

const STATE_EMPTY_WRITE_MASK: u8 = 0b11111110;

const STATE_EQUIPPED_WRITE_MASK: u8 = 0b11111111;
/// Represents whether the `OneShot` slot is empty or filled.
///
/// Used in conjunction with `AtomicU8` to track the lifecycle of the `OneShot` data.
#[repr(u8)]
enum OneShotState {
    /// No value has been sent yet.
    Empty = 0,
    /// A value has been sent and is ready to be received.
    Equipped = 1,
}

/// Core structure holding the value and state shared by the `Sender` and `Receiver`.
///
/// - `data`: Holds the actual value sent across the channel.
/// - `state`: Encodes status bits (slot state, drop tracking, and waker registration).
/// - `fut_sender`: Waker registered by the sender (if it waits for the receiver).
/// - `fut_recv`: Waker registered by the receiver (if it waits for the value).
struct OneShot<T> {
    data: UnsafeCell<MaybeUninit<T>>,
    state: AtomicU8,
    fut_sender: UnsafeCell<MaybeUninit<Waker>>,
    fut_recv: UnsafeCell<MaybeUninit<Waker>>,
}

pub struct Sender<T> {
    ptr: *mut OneShot<T>,
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        unsafe {
            let data = &*self.ptr;
            if (data.state.load(Acquire) & TX_WAKER_REG_MASK) == TX_WAKER_REG_MASK {
                (&mut *data.fut_sender.get()).assume_init_drop();
            }
            OneShot::droper(self.ptr);
        }
    }
}

pub struct Receiver<T> {
    ptr: *mut OneShot<T>,
}

/// The receiving side of a `OneShot` channel.
///
/// Dropping it will clean up the receiver's registered waker (if any) and
/// trigger deallocation when both endpoints are gone.
impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        unsafe {
            let data = &*self.ptr;
            if (data.state.load(Acquire) & RX_WAKER_REG_MASK) == RX_WAKER_REG_MASK {
                fence(Acquire);
                (&mut *data.fut_recv.get()).assume_init_drop();
            }
            OneShot::droper(self.ptr);
        }
    }
}

impl<T> OneShot<T> {
    /// Drops the `OneShot<T>` if both sender and receiver are gone.
    ///
    /// Also drops the contained value if it was written and no one is left to read it.
    #[inline]
    unsafe fn droper(ptr: *mut OneShot<T>) {
        let data = unsafe { &*ptr };
        let mut state = data.state.load(Acquire);
        let is_any_droped = (state & DROP_INDICATOR_MASK) == DROP_INDICATOR_MASK;
        if is_any_droped {
            if (state & STATE_READ_MASK) == (OneShotState::Equipped as u8) {
                unsafe {
                    (&mut *data.data.get()).assume_init_drop();
                }
            }
            // We do NOT need a `Release` store here before deallocation.
            //
            // At this point, both Tx and Rx are dropped (i.e., one of them detected that the other is already gone),
            // and this thread is about to deallocate the OneShot memory.
            //
            // Since no other thread can possibly access `state` after this point, a `Release` store would serve
            // no purpose — there is no observer left to see it.
            //
            // ⚠️ NOTE: If this logic ever changes such that:
            //   - The OneShot is pooled or reused,
            //   - Or the state is inspected after drop by another thread,
            // then this will need a `state.store(..., Release)` before deallocation
            // to guarantee visibility of the write.

            _ = unsafe { Box::from_raw(ptr) }
        } else {
            let mut new = state | DROP_INDICATOR_MASK;
            while let Err(curr_state) = data.state.compare_exchange(state, new, AcqRel, Acquire) {
                if (curr_state & DROP_INDICATOR_MASK) == DROP_INDICATOR_MASK {
                    if (curr_state & STATE_READ_MASK) == (OneShotState::Equipped as u8) {
                        unsafe {
                            fence(Acquire);
                            (&mut *data.data.get()).assume_init_drop();
                        }
                    }
                    // We do NOT need a `Release` store here before deallocation.
                    //
                    // At this point, both Tx and Rx are dropped (i.e., one of them detected that the other is already gone),
                    // and this thread is about to deallocate the OneShot memory.
                    //
                    // Since no other thread can possibly access `state` after this point, a `Release` store would serve
                    // no purpose — there is no observer left to see it.
                    //
                    // ⚠️ NOTE: If this logic ever changes such that:
                    //   - The OneShot is pooled or reused,
                    //   - Or the state is inspected after drop by another thread,
                    // then this will need a `state.store(..., Release)` before deallocation
                    // to guarantee visibility of the write.

                    _ = unsafe { Box::from_raw(ptr) }
                } else {
                    new = curr_state | DROP_INDICATOR_MASK;
                    state = curr_state;
                }
            }
        }
    }
}
pub enum Errors {
    TxDropped,
    RxDropped,
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, Errors>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let data = unsafe { &*self.ptr };
        let state = data.state.load(Acquire);

        // Registering Waker if its not registered
        if (state & RX_WAKER_REG_MASK) == 0 {
            unsafe { (&mut *data.fut_recv.get()).write(cx.waker().clone()) };
            fence(Release);
            data.state.fetch_or(RX_WAKER_REG_MASK, Release);
        }

        // If the Data is available We pop and Return
        if (state & STATE_READ_MASK) == OneShotState::Equipped as u8 {
            fence(Acquire);
            let res = unsafe { (&*data.data.get()).assume_init_read() };
            data.state.fetch_and(STATE_EMPTY_WRITE_MASK, Release);

            // TODO: Need to change the interal State of Task to Waiting so that Executor Won't put it back to ready queue to poll
            // and will be only polled when its waked
            let waker_data = unsafe { &*(cx.waker().data() as *mut WakerData) };
            waker_data.0.set_state(TaskState::Waiting);
            return Ready(Ok(res));
        }

        // Checking weather the Sender Droped or not
        if (state & DROP_INDICATOR_MASK) == DROP_INDICATOR_MASK {
            return Ready(Err(Errors::TxDropped));
        }

        // If the waker is available we Wake it
        if (state & TX_WAKER_REG_MASK) == TX_WAKER_REG_MASK {
            let waker_ref = unsafe { (&mut *data.fut_sender.get()).assume_init_ref() };
            waker_ref.wake_by_ref();
        }
        Pending
    }
}

impl<T> Sender<T> {
    pub async fn send(self: Pin<&mut Self>, data: T) {
        let poll_fun = poll_fn(move |ctx| {
            todo!()
            // Ready(())
        });
        poll_fun.await
    }
}

unsafe impl<T: Send> Send for OneShot<T> {}
unsafe impl<T: Send> Sync for OneShot<T> {}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}
