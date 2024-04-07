use crate::{Callback, Compositor, Resume};
use std::future::Future;
use tokio::sync::mpsc;

mod sealed {
    pub trait Sealed<S, E> {}
}

/// Implemented for types that can be returned from a job as a callback.
pub trait IntoCallback<S, E>: sealed::Sealed<S, E> + Send + 'static {
    fn into_callback(self) -> Option<Callback<S, E>>;
}

impl<S, E> sealed::Sealed<S, E> for () {}
impl<S, E> IntoCallback<S, E> for () {
    #[inline]
    fn into_callback(self) -> Option<Callback<S, E>> {
        None
    }
}

impl<S, E, C> sealed::Sealed<S, E> for C where C: for<'c> FnOnce(&'c mut Compositor<S, E>) + 'static {}
impl<S, E, C> IntoCallback<S, E> for C
where
    C: for<'c> FnOnce(&'c mut Compositor<S, E>) + Send + 'static,
{
    #[inline]
    fn into_callback(self) -> Option<Callback<S, E>> {
        Some(Box::new(self))
    }
}

impl<S, E, C: IntoCallback<S, E>> sealed::Sealed<S, E> for Option<C> {}
impl<S, E, C: IntoCallback<S, E>> IntoCallback<S, E> for Option<C> {
    fn into_callback(self) -> Option<Callback<S, E>> {
        self.and_then(IntoCallback::into_callback)
    }
}

/// Job system, allows to execute futures and run callbacks when job is finished.
pub struct Jobs<S, E> {
    sender: mpsc::Sender<Resume<S, E>>,
}

impl<S: 'static, E: 'static> Jobs<S, E> {
    pub(crate) fn new(sender: mpsc::Sender<Resume<S, E>>) -> Self {
        Self { sender }
    }

    pub fn spawn<C, F>(&self, job: F)
    where
        C: IntoCallback<S, E>,
        F: Future<Output = C> + Send + 'static,
        S: Send + 'static,
        E: Send + 'static,
    {
        let sender = self.sender.clone();

        tokio::spawn(async move {
            if let Some(callback) = job.await.into_callback() {
                sender
                    .send(Resume::JobCallback(callback))
                    .await
                    .expect("jobs closed");
            }
        });
    }
}
