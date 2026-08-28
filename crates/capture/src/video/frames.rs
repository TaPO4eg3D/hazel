use std::{
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex},
    task::{Poll, Waker},
};

use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer as _, Producer as _},
};

struct FrameChannelInner<T> {
    frame: Option<T>,
    waker: Option<Waker>,

    closed: bool,
}

pub(crate) struct FrameSender<T> {
    inner: Arc<Mutex<FrameChannelInner<T>>>,
}

impl<T> Drop for FrameSender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut inner = self.inner.lock().unwrap();

            inner.closed = true;
            inner.waker.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> FrameSender<T> {
    pub(crate) fn send(&self, frame: T) {
        let waker = {
            let mut inner = self.inner.lock().unwrap();

            inner.frame = Some(frame);
            inner.waker.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

pub struct FrameRecvFuture<'a, T> {
    inner: &'a Arc<Mutex<FrameChannelInner<T>>>,
}

impl<'a, T> Future for FrameRecvFuture<'a, T> {
    type Output = Option<T>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut inner = self.inner.lock().unwrap();

        if let Some(frame) = inner.frame.take() {
            return Poll::Ready(Some(frame));
        }

        if inner.closed {
            return Poll::Ready(None);
        }

        match &mut inner.waker {
            Some(waker) => waker.clone_from(cx.waker()),
            None => inner.waker = Some(cx.waker().clone()),
        }

        Poll::Pending
    }
}

pub struct FrameRecv<T> {
    inner: Arc<Mutex<FrameChannelInner<T>>>,
}

impl<T> FrameRecv<T> {
    pub fn recv<'a>(&'a mut self) -> FrameRecvFuture<'a, T> {
        FrameRecvFuture { inner: &self.inner }
    }
}

/// One-shot channel that always overrides the not yet consumed value.
/// Mainly used for previews.
///
/// Note: Should be generic and not tied to frames?
pub(crate) fn frame_channel<T>() -> (FrameSender<T>, FrameRecv<T>) {
    let inner = Arc::new(Mutex::new(FrameChannelInner {
        frame: None,
        waker: None,

        closed: false,
    }));

    (
        FrameSender {
            inner: inner.clone(),
        },
        FrameRecv {
            inner: inner.clone(),
        },
    )
}

pub struct FramePool {
    empty_frame_queue: HeapProd<Vec<u8>>,
    ready_frame_queue: HeapCons<Vec<u8>>,
}

pub struct ReadyFrame<'a> {
    data: Option<Vec<u8>>,
    empty_frame_queue: &'a mut HeapProd<Vec<u8>>,
}

impl<'a> Drop for ReadyFrame<'a> {
    fn drop(&mut self) {
        if self
            .empty_frame_queue
            .try_push(self.data.take().unwrap())
            .is_err()
        {
            todo!("handle the case");
        }
    }
}

impl<'a> Deref for ReadyFrame<'a> {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        self.data.as_ref().unwrap()
    }
}

impl<'a> DerefMut for ReadyFrame<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data.as_mut().unwrap()
    }
}

impl FramePool {
    pub(crate) fn new(empty_queue: HeapProd<Vec<u8>>, ready_queue: HeapCons<Vec<u8>>) -> Self {
        Self {
            empty_frame_queue: empty_queue,
            ready_frame_queue: ready_queue,
        }
    }

    pub fn try_get_frame(&mut self) -> Option<ReadyFrame<'_>> {
        self.ready_frame_queue.try_pop().map(|frame| ReadyFrame {
            data: Some(frame),
            empty_frame_queue: &mut self.empty_frame_queue,
        })
    }
}
