//! A thread pool for isolating blocking in async programs.
//!
//! With `mt` feature the default number of threads (set to number of cpus) can be altered
//! by setting `BLOCK_THREADS` environment variable with value.
//!

use pin_project_lite::pin_project;
use std::cmp::max;
use std::collections::VecDeque;
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{ready, Context, Poll};
use std::thread;
use std::thread::JoinHandle;

use parking_lot::{Condvar, Mutex};

#[cfg(feature = "tokio")]
mod tok {
    pub use tokio::sync::oneshot::Receiver;

    #[macro_export]
    /// create Runnable, schedule and return join
    macro_rules! runs {
        (inside $_self:ident, $f:ident, $m:ident) => {{
            let (tx, rx) = tokio::sync::oneshot::channel();

            $_self.$m(Box::new(move || {
                let _ = tx.send($f());
            }));
            Join { recv: rx }
        }};
    }
}

#[cfg(feature = "tokio")]
use self::tok::*;

#[cfg(all(feature = "kanal", not(feature = "tokio")))]
mod kan {
    pub use kanal::OneshotReceiveFuture as Receiver;

    #[macro_export]
    /// create Runnable, schedule and return join
    macro_rules! runs {
        (inside $_self:ident, $f:ident, $m:ident) => {{
            let (tx, rx) = kanal::oneshot_async();

            $_self.$m(Box::new(move || {
                let _ = tx.to_sync().send($f());
            }));
            Join { recv: rx.recv() }
        }};
    }
}
#[cfg(all(feature = "kanal", not(feature = "tokio")))]
use self::kanal::*;

/// create Runnable, schedule and return join
macro_rules! run {
    ($f:ident in $_self:ident) => {
        crate::runs!(inside $_self, $f, schedule)
    };
    ($f:ident 's in $_self:ident ) => {
        crate::runs!(inside $_self, $f, schedules)
    };
}

macro_rules! exec {
    () => {{
        let thread_limit = Executor::max_threads();
        Executor {
            queue: Mutex::new(VecDeque::with_capacity(max(thread_limit, 256))),
            thread_count: AtomicUsize::new(0),
            join: Mutex::new(Vec::with_capacity(thread_limit)),
            shutdown: AtomicBool::new(false),
            cvar: Condvar::new(),
            thread_limit,
        }
    }};
}

#[cfg(feature = "lazy")]
use once_cell::sync::Lazy;

/// Lazy initialized global executor.
#[cfg(feature = "lazy")]
static EXECUTOR: Lazy<Executor> = Lazy::new(|| exec!());

/// initialized global executor.
#[ctor::ctor]
#[cfg(not(miri))]
#[cfg(not(feature = "lazy"))]
static EXECUTOR: Executor = exec!();

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

pub trait Fun<T: Val>: FnOnce() -> T + Val {}
impl<T: Val, F: FnOnce() -> T + Val> Fun<T> for F {}

type Runnable = Box<dyn FnOnce() + Send + 'static>;

/// The unblock executor.
struct Executor {
    /// Inner queue
    queue: Mutex<VecDeque<Runnable>>,

    /// Number of spawned threads
    thread_count: AtomicUsize,

    /// Main thread waited
    join: Mutex<Vec<JoinHandle<()>>>,

    /// Shutdown threads can block all
    shutdown: AtomicBool,

    /// Used to put idle threads to sleep and wake them up when new work comes in.
    cvar: Condvar,

    /// Maximum number of threads in the pool
    thread_limit: usize,
}

struct LiveMonitor;

impl Drop for LiveMonitor {
    fn drop(&mut self) {
        if thread::panicking() {
            EXECUTOR.thread_count.fetch_sub(1, Ordering::SeqCst);
            EXECUTOR.grow_pool();
        }
    }
}

pin_project! {
    #[derive(Debug)]
    pub struct Join<T> {
        #[pin]
        recv: Receiver<T>
    }
}

impl<T> Future for Join<T> {
    type Output = Result<T, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        Poll::Ready(ready!(this.recv.poll(cx)).map_err(|_| Error))
    }
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
    fn spawns<T: Val>(&'static self, f: impl IntoIterator<Item = impl Fun<T>>) -> Vec<Join<T>> {
        let tasks = f.into_iter().map(|f| run!(f 's in self)).collect();
        self.grow_pool();
        tasks
    }

    /// Spawns a future onto this executor.
    #[inline(always)]
    fn spawn<T: Val>(&'static self, f: impl Fun<T>) -> Join<T> {
        run!(f in self)
    }

    /// Runs the main loop on the current thread.
    ///
    /// This function runs unblock tasks until it becomes idle.
    fn main_loop(&'static self) {
        let _live = LiveMonitor;
        let mut queue = self.queue.lock();
        loop {
            // Run tasks in the queue.
            while let Some(runnable) = queue.pop_front() {
                drop(queue);
                runnable();
                queue = self.queue.lock();
            }

            if self.shutdown.load(Ordering::Relaxed) {
                break;
            }

            // Put the thread to sleep until another task is scheduled.
            self.cvar.wait(&mut queue);
        }
    }
    /// Schedules a runnable task for execution.
    #[inline(always)]
    fn schedules(&'static self, runnable: Runnable) {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }
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
        while self.thread_count.load(Ordering::SeqCst) < self.thread_limit
            && !self.shutdown.load(Ordering::Relaxed)
        {
            let id = self.thread_count.fetch_add(1, Ordering::Relaxed);

            // Spawn the new thread.
            self.join.lock().push(
                thread::Builder::new()
                    .name(format!("unblock-{}", id))
                    .spawn(move || self.main_loop())
                    .unwrap(),
            );
        }
    }

    /// Put executor in shutdown
    fn drop(&'static self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.queue.lock().drain(..);
        self.cvar.notify_all();
        for j in self.join.lock().drain(..) {
            let _ = j.join();
        }
    }
}

#[ctor::dtor]
fn des() {
    EXECUTOR.drop();
}

/// Runs unblock code on a thread pool and return a future
pub fn unblock<T: Val>(f: impl Fun<T>) -> Join<T> {
    EXECUTOR.spawn(f)
}

/// Runs multiple unblock code on a thread pool and return futures in order
pub fn unblocks<T: Val>(f: impl IntoIterator<Item = impl Fun<T>>) -> Vec<Join<T>> {
    EXECUTOR.spawns(f)
}
