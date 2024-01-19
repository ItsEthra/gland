use crate::{Callback, Compositor, Resume};
use std::{future::Future, marker::PhantomData};
use tokio::{sync::mpsc, task::LocalSet};

mod sealed {
    pub trait Sealed<S, E> {}
}

pub trait IntoCallback<S, E>: sealed::Sealed<S, E> + 'static {
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
    C: for<'c> FnOnce(&'c mut Compositor<S, E>) + 'static,
{
    #[inline]
    fn into_callback(self) -> Option<Callback<S, E>> {
        Some(Box::new(self))
    }
}

pub struct Jobs<'set, S, E> {
    pub(crate) set: &'set LocalSet,
    sender: mpsc::Sender<Resume<S, E>>,
    _se: PhantomData<(S, E)>,
}

impl<'set, S: 'static, E: 'static> Jobs<'set, S, E> {
    pub(crate) fn new(set: &'set LocalSet, sender: mpsc::Sender<Resume<S, E>>) -> Self {
        Self {
            set,
            sender,
            _se: PhantomData,
        }
    }

    pub fn spawn<C, F>(&self, job: F)
    where
        C: IntoCallback<S, E>,
        F: Future<Output = C> + 'static,
    {
        let sender = self.sender.clone();
        self.set.spawn_local(async move {
            if let Some(callback) = job.await.into_callback() {
                sender
                    .send(Resume::JobCallback(callback))
                    .await
                    .expect("jobs closed");
            }
        });
    }
}
