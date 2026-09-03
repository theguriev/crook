//! The two executors the app runs on.
//!
//! [`Foreground`] runs futures on the thread that owns the [`crate::App`], so a
//! task can touch entities directly. It cannot run them itself — only the
//! platform's event loop can — so it is constructed with a scheduler that hands
//! a ready task back to that loop (a winit `EventLoopProxy`, say).
//!
//! [`Background`] is a plain worker pool for work that must not block a frame.
//! Nothing it runs may touch an entity; results come home by awaiting the task
//! from a foreground future.
//!
//! Both are thin wrappers over `async-task`, which is the whole of the
//! machinery: a future plus a scheduler produce a [`Runnable`] (call it to
//! poll) and a [`Task`] (await it for the output).

use std::collections::VecDeque;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Waker;
use std::thread::JoinHandle;

pub use async_task::{Runnable, Task};
use parking_lot::{Condvar, Mutex};

/// The main-thread executor.
///
/// Deliberately `!Send`: the futures it spawns are not `Send` either, and
/// `async-task` will panic at run time if one is polled on another thread.
pub struct Foreground {
    schedule: Arc<dyn Fn(Runnable) + Send + Sync>,
    not_send_or_sync: PhantomData<Rc<()>>,
}

impl Foreground {
    /// Builds an executor that hands ready tasks to `schedule`.
    ///
    /// `schedule` is called from whichever thread woke the task, so it must be
    /// `Send + Sync`; its job is to get the [`Runnable`] back onto the main
    /// thread and call it there.
    pub fn new(schedule: impl 'static + Fn(Runnable) + Send + Sync) -> Self {
        Self {
            schedule: Arc::new(schedule),
            not_send_or_sync: PhantomData,
        }
    }

    /// Schedules `future` to run on the main thread.
    ///
    /// Dropping the returned [`Task`] cancels the future; call
    /// [`Task::detach`] to let it run to completion unattended.
    pub fn spawn<T: 'static>(&self, future: impl 'static + Future<Output = T>) -> Task<T> {
        let schedule = self.schedule.clone();
        let (runnable, task) = async_task::spawn_local(future, move |runnable| schedule(runnable));
        runnable.schedule();
        task
    }
}

/// A main-thread queue that drives a [`Foreground`] from the current thread.
///
/// This is what a test uses in place of an event loop, and what a platform with
/// no proxy of its own can pump each frame.
#[derive(Default)]
pub struct LocalQueue {
    runnables: Mutex<VecDeque<Runnable>>,
    waker: Mutex<Option<Waker>>,
}

impl LocalQueue {
    /// An empty queue.
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// A [`Foreground`] whose ready tasks land in this queue.
    pub fn foreground(self: &Arc<Self>) -> Rc<Foreground> {
        let queue = self.clone();
        Rc::new(Foreground::new(move |runnable| queue.push(runnable)))
    }

    /// Runs every task that is currently ready, and any they make ready.
    ///
    /// Returns how many ran, which is how a caller can tell whether pumping
    /// again could make more progress.
    pub fn run_until_parked(&self) -> usize {
        let mut ran = 0;
        loop {
            // `let … else` rather than `while let`, so the guard is dropped at
            // the end of *this statement* rather than at the end of the loop
            // body. A `while let` holds it across `run()`, and a task that
            // schedules another foreground task — which is what every
            // self-rescheduling poll chain does from its own completion —
            // then deadlocks on [`Self::push`].
            let Some(runnable) = self.runnables.lock().pop_front() else {
                return ran;
            };
            runnable.run();
            ran += 1;
        }
    }

    /// Blocks the current thread on `future`, running queued tasks whenever it
    /// would otherwise be idle.
    pub fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        futures_lite::future::block_on(async {
            let mut future = pin!(future);
            std::future::poll_fn(|ctx| {
                // Registering before draining is what makes a task scheduled
                // *during* the drain wake this thread instead of stranding it.
                *self.waker.lock() = Some(ctx.waker().clone());
                self.run_until_parked();
                future.as_mut().poll(ctx)
            })
            .await
        })
    }

    fn push(&self, runnable: Runnable) {
        self.runnables.lock().push_back(runnable);
        if let Some(waker) = self.waker.lock().take() {
            waker.wake();
        }
    }
}

/// A pool of worker threads for work that must not block a frame.
pub struct Background {
    shared: Arc<BackgroundShared>,
    workers: Vec<JoinHandle<()>>,
}

#[derive(Default)]
struct BackgroundShared {
    runnables: Mutex<VecDeque<Runnable>>,
    ready: Condvar,
    is_shutting_down: AtomicBool,
}

impl Default for Background {
    fn default() -> Self {
        Self::new(
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1),
        )
    }
}

impl Background {
    /// Starts `thread_count` workers, at least one.
    pub fn new(thread_count: usize) -> Self {
        let shared = Arc::new(BackgroundShared::default());
        let workers = (0..thread_count.max(1))
            .map(|index| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("crook-background-{index}"))
                    .spawn(move || shared.run_worker())
                    .expect("the OS refused to start a background thread")
            })
            .collect();

        Self { shared, workers }
    }

    /// Schedules `future` onto a worker thread.
    ///
    /// Dropping the returned [`Task`] cancels the future; await it from a
    /// foreground task to bring the result back to the main thread.
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Send + 'static + Future<Output = T>,
    ) -> Task<T> {
        let shared = self.shared.clone();
        let (runnable, task) = async_task::spawn(future, move |runnable| shared.push(runnable));
        runnable.schedule();
        task
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        {
            // Under the lock, and that is the whole of it. A worker holds this
            // mutex from the moment it reads the flag until `wait` atomically
            // releases it and parks, so taking it here means the flag can only
            // be set while every worker is either parked — and so will be woken
            // — or not yet looking. Setting it outside the lock leaves the
            // window in between: `notify_all` reaches nobody, the worker parks
            // a moment later against a flag that is already true, and the
            // `join` below never returns.
            let _shutting_down = self.shared.runnables.lock();
            self.shared.is_shutting_down.store(true, Ordering::Release);
        }
        self.shared.ready.notify_all();

        for worker in self.workers.drain(..) {
            // A worker only blocks on the condvar, so this cannot deadlock —
            // but a task that blocks forever would hold shutdown up, which is
            // the price of joining rather than detaching.
            let _ = worker.join();
        }
    }
}

impl BackgroundShared {
    fn run_worker(&self) {
        loop {
            let runnable = {
                let mut runnables = self.runnables.lock();
                loop {
                    if self.is_shutting_down.load(Ordering::Acquire) {
                        return;
                    }
                    match runnables.pop_front() {
                        Some(runnable) => break runnable,
                        None => self.ready.wait(&mut runnables),
                    }
                }
            };
            runnable.run();
        }
    }

    fn push(&self, runnable: Runnable) {
        self.runnables.lock().push_back(runnable);
        self.ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// How long the shutdown test waits before calling it a hang.
    ///
    /// Generous, because it is competing with every other test in the binary
    /// for cores; the failure it is looking for is unbounded, not slow.
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

    #[test]
    fn a_pool_dropped_before_its_workers_park_still_shuts_them_down() {
        // The lost wake-up. A worker reads the shutdown flag, finds it false,
        // and is about to park; the flag is set and `notify_all` fires in that
        // instant, reaching nobody; the worker then sleeps forever against a
        // flag that is already true and `join` never returns. Creating and
        // dropping a pool immediately is what makes the window as wide as it
        // gets, and several workers is what makes hitting it likely.
        //
        // Run on a thread of its own so the failure is a failing test rather
        // than a test binary that hangs.
        let (shut_down, finished) = mpsc::channel();
        std::thread::spawn(move || {
            for _ in 0..200 {
                drop(Background::new(4));
            }
            let _ = shut_down.send(());
        });

        finished
            .recv_timeout(SHUTDOWN_TIMEOUT)
            .expect("dropping a pool hung joining a worker that never woke");
    }

    #[test]
    fn foreground_tasks_run_on_the_queue_that_scheduled_them() {
        let queue = LocalQueue::new();
        let foreground = queue.foreground();
        let ran = Rc::new(Cell::new(false));

        let task = {
            let ran = ran.clone();
            foreground.spawn(async move { ran.set(true) })
        };
        assert!(!ran.get(), "nothing runs until the queue is pumped");

        queue.run_until_parked();
        assert!(ran.get());
        queue.block_on(task);
    }

    #[test]
    fn a_task_that_schedules_another_does_not_deadlock_the_queue() {
        // Every self-rescheduling poll chain does this: the cycle's completion
        // callback spawns the next cycle. A queue that held its lock across a
        // task's run would deadlock on the very first one.
        let queue = LocalQueue::new();
        let foreground = queue.foreground();
        let ran = Rc::new(Cell::new(0));

        let spawn_one = {
            let ran = ran.clone();
            let queue_foreground = foreground.clone();
            foreground.spawn(async move {
                ran.set(ran.get() + 1);
                let ran = ran.clone();
                queue_foreground
                    .spawn(async move { ran.set(ran.get() + 1) })
                    .detach();
            })
        };
        spawn_one.detach();

        // Both, in one pump: the second was made ready by the first.
        assert_eq!(queue.run_until_parked(), 2);
        assert_eq!(ran.get(), 2);
    }

    #[test]
    fn a_background_result_can_be_awaited_from_the_foreground() {
        let queue = LocalQueue::new();
        let background = Background::new(2);

        let doubled = background.spawn(async { 21 * 2 });
        assert_eq!(queue.block_on(doubled), 42);
    }
}
