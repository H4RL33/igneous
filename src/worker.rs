//! A thread that owns something not shareable with the main loop (the Git
//! runner, the index's database) and runs jobs against it one at a time.

use std::sync::mpsc;

type Job<T> = Box<dyn FnOnce(&mut T) + Send>;

pub struct Worker<T> {
    sender: mpsc::Sender<Job<T>>,
}

impl<T> Clone for Worker<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<T: 'static> Worker<T> {
    /// Starts a thread called `name` that builds its resource with `make`.
    pub fn new(name: &str, make: impl FnOnce() -> T + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel::<Job<T>>();
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                let mut resource = make();
                for job in receiver {
                    job(&mut resource);
                }
            })
            .expect("can start a worker thread");
        Self { sender }
    }

    /// Queues `f` on the thread now (so it runs before anything queued
    /// later) and returns where its result will arrive.
    pub fn submit<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut T) -> R + Send + 'static,
    ) -> async_channel::Receiver<R> {
        let (sender, receiver) = async_channel::bounded(1);
        let _ = self.sender.send(Box::new(move |resource| {
            let _ = sender.send_blocking(f(resource));
        }));
        receiver
    }

    /// Runs `f` on the thread. `None` if the thread has gone.
    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut T) -> R + Send + 'static,
    ) -> Option<R> {
        let (sender, receiver) = async_channel::bounded(1);
        self.sender
            .send(Box::new(move |resource| {
                let _ = sender.send_blocking(f(resource));
            }))
            .ok()?;
        receiver.recv().await.ok()
    }
}
