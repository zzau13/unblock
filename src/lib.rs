//! A thread pool for isolating blocking in async programs.
//!
//! # Examples
//!
//! Read the contents of a file:
//!
//! ```
//! use blocking::unblock;
//! use std::fs;
//!
//! # futures_lite::future::block_on(async {
//! let contents = unblock(|| fs::read_to_string("file.txt")).await?;
//! println!("{:?}", contents);
//! # std::io::Result::Ok(()) });
//! ```
//!

use std::collections::VecDeque;
use std::fmt::Formatter;
use std::future::Future;
use std::panic::UnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use std::{env, panic};

use async_oneshot::oneshot;
use once_cell::sync::Lazy;

/// Default value for max threads that Executor can grow to
const DEFAULT_MAX_THREADS: usize = 500;

/// Minimum value for max threads config
const MIN_MAX_THREADS: usize = 1;

/// Maximum value for max threads config
const MAX_MAX_THREADS: usize = 10000;

/// Env variable that allows to override default value for max threads.
const MAX_THREADS_ENV: &str = "BLOCKING_MAX_THREADS";

/// Lazily initialized global executor.
static EXECUTOR: Lazy<Executor> = Lazy::new(|| Executor {
    inner: Mutex::new(Inner {
        idle_count: 0,
        thread_count: 0,
        queue: VecDeque::new(),
    }),
    cvar: Condvar::new(),
    thread_limit: Executor::max_threads(),
});

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Error;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        "Error".fmt(f)
    }
}

impl From<Error> for std::io::Error {
    fn from(_: Error) -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::Other)
    }
}

pub trait Task<T: Send + 'static>: Future<Output = Result<T, Error>> {}
impl<T: Send + 'static, F: Future<Output = Result<T, Error>>> Task<T> for F {}

pub trait Fun<T: Send + 'static>: FnOnce() -> T + Send + UnwindSafe + 'static {}
impl<T: Send + 'static, F: FnOnce() -> T + Send + UnwindSafe + 'static> Fun<T> for F {}

/// The blocking executor.
struct Executor {
    /// Inner state of the executor.
    inner: Mutex<Inner>,

    /// Used to put idle threads to sleep and wake them up when new work comes in.
    cvar: Condvar,

    /// Maximum number of threads in the pool
    thread_limit: usize,
}

/// Inner state of the blocking executor.
struct Inner {
    /// Number of idle threads in the pool.
    ///
    /// Idle threads are sleeping, waiting to get a task to run.
    idle_count: usize,

    /// Total number of threads in the pool.
    ///
    /// This is the number of idle threads + the number of active threads.
    thread_count: usize,

    /// The queue of blocking tasks.
    queue: VecDeque<Box<dyn FnOnce() + Send + 'static>>,
}

impl Executor {
    fn max_threads() -> usize {
        match env::var(MAX_THREADS_ENV) {
            Ok(v) => v
                .parse::<usize>()
                .map(|v| v.max(MIN_MAX_THREADS).min(MAX_MAX_THREADS))
                .unwrap_or(DEFAULT_MAX_THREADS),
            Err(_) => DEFAULT_MAX_THREADS,
        }
    }

    /// Spawns a future onto this executor.
    ///
    /// Returns a [`Task`] handle for the spawned task.
    fn spawn<T: Send + Sync + 'static>(f: impl Fun<T>) -> impl Task<T> {
        let (mut tx, rx) = oneshot();
        EXECUTOR.schedule::<fn()>(Box::new(move || {
            let r = panic::catch_unwind(f);
            let _ = tx.send(r.map_err(|_| Error));
        }));
        async move {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err(Error),
            }
        }
    }

    /// Runs the main loop on the current thread.
    ///
    /// This function runs blocking tasks until it becomes idle and times out.
    fn main_loop(&'static self) {
        let mut inner = self.inner.lock().unwrap();
        loop {
            // This thread is not idle anymore because it's going to run tasks.
            inner.idle_count -= 1;

            // Run tasks in the queue.
            while let Some(runnable) = inner.queue.pop_front() {
                // We have found a task - grow the pool if needed.
                self.grow_pool(inner);

                // Run the task.
                runnable();

                // Re-lock the inner state and continue.
                inner = self.inner.lock().unwrap();
            }

            // This thread is now becoming idle.
            inner.idle_count += 1;

            // Put the thread to sleep until another task is scheduled.
            let timeout = Duration::from_millis(500);
            let (lock, res) = self.cvar.wait_timeout(inner, timeout).unwrap();
            inner = lock;

            // If there are no tasks after a while, stop this thread.
            if res.timed_out() && inner.queue.is_empty() {
                inner.idle_count -= 1;
                inner.thread_count -= 1;
                break;
            }
        }
    }

    /// Schedules a runnable task for execution.
    fn schedule<F>(&'static self, runnable: Box<dyn FnOnce() + Send + 'static>) {
        let mut inner = self.inner.lock().unwrap();
        inner.queue.push_back(runnable);

        // Notify a sleeping thread and spawn more threads if needed.
        self.cvar.notify_one();
        self.grow_pool(inner);
    }

    /// Spawns more blocking threads if the pool is overloaded with work.
    fn grow_pool(&'static self, mut inner: MutexGuard<'static, Inner>) {
        // If runnable tasks greatly outnumber idle threads and there aren't too many threads
        // already, then be aggressive: wake all idle threads and spawn one more thread.
        while inner.queue.len() > inner.idle_count * 5 && inner.thread_count < EXECUTOR.thread_limit
        {
            // The new thread starts in idle state.
            inner.idle_count += 1;
            inner.thread_count += 1;

            // Notify all existing idle threads because we need to hurry up.
            self.cvar.notify_all();

            // Generate a new thread ID.
            static ID: AtomicUsize = AtomicUsize::new(1);
            let id = ID.fetch_add(1, Ordering::Relaxed);

            // Spawn the new thread.
            thread::Builder::new()
                .name(format!("blocking-{}", id))
                .spawn(move || self.main_loop())
                .unwrap();
        }
    }
}

/// Runs blocking code on a thread pool.
///
/// # Examples
///
/// Read the contents of a file:
///
/// ```
/// use blocking::unblock;
/// use std::fs;
///
/// # futures_lite::future::block_on(async {
/// let contents = unblock(|| fs::read_to_string("file.txt")).await?;
/// # std::io::Result::Ok(()) });
/// ```
///
/// Spawn a process:
///
/// ```no_run
/// use blocking::unblock;
/// use std::process::Command;
///
/// # futures_lite::future::block_on(async {
/// let out = unblock(|| Command::new("dir").output()).await?;
/// # std::io::Result::Ok(()) });
/// ```
// TODO: Sync is needed by oneshot but can be without it
pub fn unblock<T: Send + Sync + 'static, F: Fun<T>>(f: F) -> impl Task<T> {
    Executor::spawn(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_max_threads() {
        // properly set env var
        env::set_var(MAX_THREADS_ENV, "100");
        assert_eq!(100, Executor::max_threads());

        // passed value below minimum, so we set it to minimum
        env::set_var(MAX_THREADS_ENV, "0");
        assert_eq!(1, Executor::max_threads());

        // passed value above maximum, so we set to allowed maximum
        env::set_var(MAX_THREADS_ENV, "50000");
        assert_eq!(10000, Executor::max_threads());

        // no env var, use default
        env::set_var(MAX_THREADS_ENV, "");
        assert_eq!(500, Executor::max_threads());

        // not a number, use default
        env::set_var(MAX_THREADS_ENV, "NOTINT");
        assert_eq!(500, Executor::max_threads());
    }

    #[test]
    fn test_sleep() {
        futures_lite::future::block_on(async {
            assert_eq!(
                unblock(|| {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    "foo"
                })
                .await
                .unwrap(),
                "foo"
            )
        });
    }
    #[test]
    fn test_panic() {
        futures_lite::future::block_on(async {
            assert!(unblock(|| {
                panic!("");
            })
            .await
            .is_err())
        });
    }
}
