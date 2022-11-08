//! A thread pool for isolating blocking in async programs.
//!
//! With `mt` feature the default number of threads (set to number of cpus) can be altered
//! by setting `BLOCK_THREADS` environment variable with value.
//!
//! # Examples
//!
//! Read the contents of a file:
//!
//! ```
//! use std::fs;
//!
//! use unblock::unblock;
//!
//! # futures::executor::block_on(async {
//! let contents = unblock(|| fs::read_to_string("file.txt")).await?;
//! println!("{:?}", contents);
//! # std::io::Result::Ok(()) });
//! ```
//!

use std::cmp::max;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::panic;
use std::panic::UnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use once_cell::sync::Lazy;
use parking_lot::{Condvar, Mutex};
use tokio::sync::oneshot::channel as oneshot;

/// Lazily initialized global executor.
static EXECUTOR: Lazy<Executor> = Lazy::new(|| {
    let thread_limit = Executor::max_threads();
    Executor {
        queue: Mutex::new(VecDeque::with_capacity(max(thread_limit, 256))),
        thread_count: AtomicUsize::new(0),
        cvar: Condvar::new(),
        thread_limit,
    }
});

/// No-size error
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Error;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        "Error".fmt(f)
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(_: Error) -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::Other)
    }
}

pub trait Val: Send + 'static {}
impl<T: Send + 'static> Val for T {}

pub trait Task<T: Val>: Future<Output = Result<T, Error>> {}
impl<T: Val, F: Future<Output = Result<T, Error>>> Task<T> for F {}

pub trait Fun<T: Val>: FnOnce() -> T + UnwindSafe + Val {}
impl<T: Val, F: FnOnce() -> T + UnwindSafe + Val> Fun<T> for F {}

type Runnable = Box<dyn FnOnce() + Send + 'static>;

/// The unblock executor.
struct Executor {
    /// Inner queue
    queue: Mutex<VecDeque<Runnable>>,

    /// Number of spawned threads
    thread_count: AtomicUsize,

    /// Used to put idle threads to sleep and wake them up when new work comes in.
    cvar: Condvar,

    /// Maximum number of threads in the pool
    thread_limit: usize,
}

/// create Runnable, schedule and return join
macro_rules! runnable {
    ($_self:ident spawn $f:ident) => {
        runnable!(inside $_self, $f, schedule)
    };
    ($_self:ident spawns $f:ident) => {
        runnable!(inside $_self, $f, schedules)
    };
    (inside $_self:ident, $f:ident, $m:ident) => {{
        let (tx, rx) = oneshot();

        $_self.$m(Box::new(move || {
            let r = panic::catch_unwind($f);
            let _ = tx.send(r.map_err(|_| Error));
        }));
        async move {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err(Error),
            }
        }
    }};
}

impl Executor {
    #[inline(always)]
    fn max_threads() -> usize {
        #[allow(unused_mut, unused_assignments)]
        let mut threads = 1usize;
        #[cfg(feature = "mt")]
        {
            threads = match std::env::var("BLOCK_THREADS")
                .ok()
                .and_then(|x| x.parse().ok())
            {
                Some(num_cpus) => num_cpus,
                None => num_cpus::get(),
            };
        };

        threads
    }

    /// Spawns futures onto this executor.
    #[inline(always)]
    fn spawns<T: Val>(
        &'static self,
        f: impl IntoIterator<Item = impl Fun<T>>,
    ) -> Vec<impl Task<T>> {
        let tasks = f.into_iter().map(|f| runnable!(self spawns f)).collect();
        self.grow_pool();
        tasks
    }

    /// Spawns a future onto this executor.
    #[inline(always)]
    fn spawn<T: Val>(&'static self, f: impl Fun<T>) -> impl Task<T> {
        runnable!(self spawn f)
    }

    /// Runs the main loop on the current thread.
    ///
    /// This function runs unblock tasks until it becomes idle.
    fn main_loop(&'static self) {
        let mut queue = self.queue.lock();
        loop {
            // Run tasks in the queue.
            while let Some(runnable) = queue.pop_front() {
                drop(queue);
                runnable();
                queue = self.queue.lock();
            }

            // Put the thread to sleep until another task is scheduled.
            self.cvar.wait(&mut queue);
        }
    }
    /// Schedules a runnable task for execution.
    #[inline(always)]
    fn schedules(&'static self, runnable: Runnable) {
        self.queue.lock().push_back(runnable);

        // Notify a sleeping thread
        self.cvar.notify_one();
    }

    /// Schedules a runnable task for execution and grow thread pool if needed
    #[inline(always)]
    fn schedule(&'static self, runnable: Runnable) {
        self.schedules(runnable);
        // spawn more threads if needed.
        self.grow_pool();
    }

    /// Spawns more block threads
    #[inline(always)]
    fn grow_pool(&'static self) {
        while self.thread_count.load(Ordering::SeqCst) < self.thread_limit {
            let id = self.thread_count.fetch_add(1, Ordering::Relaxed);

            // Spawn the new thread.
            thread::Builder::new()
                .name(format!("unblock-{}", id))
                .spawn(move || self.main_loop())
                .unwrap();
        }
    }
}

/// Runs unblock code on a thread pool and return a future
///
/// # Examples
///
/// Read the contents of a file:
///
/// ```
/// use unblock::unblock;
/// use std::fs;
///
/// # futures::executor::block_on(async {
/// let contents = unblock(|| fs::read_to_string("file.txt")).await?;
/// # std::io::Result::Ok(()) });
/// ```
///
/// Spawn a process:
///
/// ```
/// # #[cfg(not(miri))]
/// # {
/// use unblock::unblock;
/// use std::process::Command;
///
/// # futures::executor::block_on(async {
/// let out = unblock(|| Command::new("echo").arg("foo").output()).await??.stdout;
/// assert_eq!(out, b"foo\n");
/// # std::io::Result::Ok(()) });
/// # }
/// ```
pub fn unblock<T: Val>(f: impl Fun<T>) -> impl Task<T> {
    EXECUTOR.spawn(f)
}

/// Runs multiple unblock code on a thread pool and return futures in order
///
/// Read the contents of files:
///
/// ```
/// use unblock::unblocks;
/// use std::fs;
///
///
/// # futures::executor::block_on(async { ///
/// for result in unblocks(["name.txt", "foo.txt"].map(|name| move || fs::read_to_string(name))) {
///     println!("{}", result.await??);
/// }
/// # std::io::Result::Ok(()) });
/// ```
///
pub fn unblocks<T: Val>(f: impl IntoIterator<Item = impl Fun<T>>) -> Vec<impl Task<T>> {
    EXECUTOR.spawns(f)
}
