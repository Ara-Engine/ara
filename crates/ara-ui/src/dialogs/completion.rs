use std::{
    cell::Cell,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

struct Completion<T> {
    value: Cell<Option<T>>,
    waker: Cell<Option<Waker>>,
}

/// The eventual response to a native dialog. Await it on the UI thread; closing
/// the controller also resolves every outstanding request as cancelled.
pub struct DialogFuture<T> {
    state: Rc<Completion<T>>,
}

impl<T> DialogFuture<T> {
    /// Take a completed response without an executor. A host can poll this after
    /// dispatching dialog input; None means the request has not completed yet.
    pub fn try_take(&mut self) -> Option<T> {
        let value = self.state.value.take();
        if value.is_some() {
            self.state.waker.take();
        }
        value
    }
}

impl<T> Future for DialogFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T> {
        if let Some(value) = self.state.value.take() {
            Poll::Ready(value)
        } else {
            self.state.waker.set(Some(context.waker().clone()));
            Poll::Pending
        }
    }
}

pub(super) struct Reply<T> {
    state: Rc<Completion<T>>,
}

impl<T> Reply<T> {
    pub(super) fn new() -> (Self, DialogFuture<T>) {
        let state = Rc::new(Completion {
            value: Cell::new(None),
            waker: Cell::new(None),
        });
        (
            Self {
                state: Rc::clone(&state),
            },
            DialogFuture { state },
        )
    }

    pub(super) fn finish(self, value: T) {
        self.state.value.set(Some(value));
        if let Some(waker) = self.state.waker.take() {
            waker.wake();
        }
    }
}
