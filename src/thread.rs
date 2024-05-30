/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};

pub(crate) struct AsyncJoinHandle<T>
where
    T: Send + 'static,
{
    join_handle: Option<JoinHandle<T>>,
    context: Arc<Mutex<JoinContext>>,
}

struct JoinContext {
    waker: Option<Waker>,
    exited: bool,
}

struct JoinGuard {
    context: Arc<Mutex<JoinContext>>,
}

impl<T: Send> Future for AsyncJoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let this = Pin::into_inner(self);
        let guard = this.context.lock();
        let mut context = guard.unwrap();
        context.waker.replace(cx.waker().clone());
        if let Some(join_handle) = this.join_handle.as_mut() {
            if join_handle.is_finished() || context.exited {
                let join_handle = this.join_handle.take().unwrap();
                return Poll::Ready(join_handle.join().unwrap());
            }
        }
        Poll::Pending
    }
}

impl Drop for JoinGuard {
    fn drop(&mut self) {
        let guard = self.context.lock();
        let mut context = guard.unwrap();
        context.exited = true;
        let waker = context.waker.take();
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

pub(crate) fn spawn<F, T>(f: F) -> AsyncJoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let context = Arc::new(Mutex::new(JoinContext {
        waker: None,
        exited: false,
    }));

    let thread_context = context.clone();
    let join_handle = Some(thread::spawn(move || {
        let _guard = JoinGuard {
            context: thread_context,
        };
        f()
    }));

    AsyncJoinHandle {
        join_handle,
        context,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::thread::sleep as sync_sleep;
    use std::time::Duration;
    use tokio::time::sleep as async_sleep;

    #[tokio::test]
    async fn test_join() {
        let handle = spawn(|| true);
        assert!(handle.await);
    }

    #[tokio::test]
    async fn test_slow_join() {
        let handle = spawn(|| true);
        async_sleep(Duration::from_millis(100)).await;
        assert!(handle.await);
    }

    #[tokio::test]
    async fn test_slow_thread() {
        let handle = spawn(|| {
            sync_sleep(Duration::from_millis(100));
            true
        });
        assert!(handle.await);
    }
}
