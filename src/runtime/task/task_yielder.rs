use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub(super) struct Yielder {
    is_yielded: bool,
}
impl Yielder {
    pub fn new() -> Self {
        Self { is_yielded: false }
    }
}
impl Future for Yielder {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.is_yielded {
            Poll::Ready(())
        } else {
            // If pending the executor will automatically insert into Queue Again
            self.as_mut().get_mut().is_yielded = true;
            Poll::Pending
        }
    }
}
pub async fn yield_now() {
    let yielder = Yielder::new();
    yielder.await
}
