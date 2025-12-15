use std::error;
use std::fmt;

/// An error returned from the [`set_pool_size`] method.
///
/// [`set_pool_size`]: super::thread::ThreadPool::set_pool_size
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ThreadPoolError {
    /// Pool size could not be set because the threads in the thread pool have termianted.
    ThreadsLost,

    /// Pool size cannot be set to zero.
    ValueError,
}

impl fmt::Debug for ThreadPoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreadPoolError::ThreadsLost => "ThreadPoolError::ThreadsLost".fmt(f),
            ThreadPoolError::ValueError => "ThreadPoolError::ValueError".fmt(f),
        }
    }
}

impl fmt::Display for ThreadPoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThreadPoolError::ThreadsLost => "thread pool lost".fmt(f),
            ThreadPoolError::ValueError => "cannot set thread pool size to 0".fmt(f),
        }
    }
}

impl error::Error for ThreadPoolError {}
