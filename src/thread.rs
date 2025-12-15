use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{spawn, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, select, select_biased, tick};
use genzero;
use quanta::Clock;

use super::channel::*;
use super::err::ThreadPoolError;

const UPDATE_SIZE: u8 = 0;
const STOP_THREAD: u8 = 1;

/// Struct containing thread pool metrics, that is updated every approx. every 10 seconds.
#[derive(Default, Clone, Copy)]
pub struct Metrics {
    /// Current number of running threads, which may temporarily differ from [`ThreadPool::get_pool_size`].
    pub active_threads: usize,
    pub input_channel_len: usize,
    pub input_channel_capacity: Option<usize>,
    pub output_channel_len: usize,
    pub output_channel_capacity: Option<usize>,

    /// Total executions of the provided function since the last metrics update.
    /// Inputs that have been executed on, but not sent are not counted until they have sent;
    pub execution_count: usize,
    /// Average nano-second execution duration of the provided function since the last metrics update.
    /// Does not include any time the input/output spends in the channels.
    pub average_execution_duration_ns: usize,
    // pub minimum_execution_duration_ns: usize,
    // pub maximum_execution_duration_ns: usize,
}

struct ExecutionMetrics {
    clock: Clock,
    execution_counter: AtomicUsize,
    total_execution_time_ns: AtomicUsize,
    // min_execution_time_ns: AtomicUsize,
    // max_execution_time_ns: AtomicUsize,
}

impl ExecutionMetrics {
    fn new() -> Self {
        ExecutionMetrics {
            clock: Clock::new(),
            execution_counter: AtomicUsize::new(0),
            total_execution_time_ns: AtomicUsize::new(0),
        }
    }

    fn update(&self, execution_time: usize) {
        self.execution_counter.fetch_add(1, Ordering::Relaxed);
        self.total_execution_time_ns
            .fetch_add(execution_time, Ordering::Relaxed);
    }

    fn get_and_reset_execution_count(&self) -> usize {
        self.execution_counter.fetch_and(0, Ordering::Relaxed)
    }

    fn get_and_reset_total_execution_time_ns(&self) -> usize {
        self.total_execution_time_ns.fetch_and(0, Ordering::Relaxed)
    }
}

/// The thread pool of a lambda-channel that spawns threads running an infinite loop.
/// The thread loop waits for a message from the input channel, executes a provided function,
/// and sends the result to the output channel if the function has an output.
///
/// The pool starts with one control thread, all additional threads are normal worker threads.
/// The control thread is identical to a normal worker thread, but also handles metrics collection, pool resizing, and termination propagation.
///
/// If the pool is dropped, the threads will automatically terminate.
/// If the input channel or output channel to the thread pool disconnects, the threads will automatically terminate.
/// If the thread is executing on an input or waiting to send an output when termination is triggered, it will finish execution and send the
/// output value before terminating.
#[derive(Clone)]
pub struct ThreadPool {
    desired_threads: Arc<AtomicUsize>,
    control_tx: crossbeam_channel::Sender<u8>,
    metrics_rx: genzero::Receiver<Metrics>,
}

impl ThreadPool {
    pub(super) fn new_lambda_pool<
        T: Send + 'static,
        U: Send + 'static,
        V: Clone + Send + 'static,
    >(
        input_channel: Receiver<T>,
        output_channel: Sender<U>,
        shared_resource: V,
        function: fn(&V, T) -> U,
    ) -> Self {
        let desired_threads = Arc::new(AtomicUsize::new(1));
        let (control_tx, metrics_rx) = spawn_primary_lambda_thread(
            input_channel,
            output_channel,
            shared_resource,
            function,
            desired_threads.clone(),
        );

        Self {
            desired_threads,
            control_tx,
            metrics_rx,
        }
    }

    pub(super) fn new_sink_pool<T: Send + 'static, V: Clone + Send + 'static>(
        input_channel: Receiver<T>,
        shared_resource: V,
        function: fn(&V, T),
    ) -> Self {
        let desired_threads = Arc::new(AtomicUsize::new(1));
        let (control_tx, metrics_rx) = spawn_primary_sink_thread(
            input_channel,
            shared_resource,
            function,
            desired_threads.clone(),
        );

        Self {
            desired_threads,
            control_tx,
            metrics_rx,
        }
    }

    /// Returns the target number of threads in the pool. The actual number of threads may temporarily differ
    /// when resizing the pool.
    pub fn get_pool_size(&self) -> usize {
        self.desired_threads.load(Ordering::Acquire)
    }

    /// Sets the target number of threads in the pool. The actual number of threads may temporarily differ
    /// when resizing the pool.
    ///
    /// This function will return the desired thread size if successful, or a [`ThreadPoolError`] if not.
    pub fn set_pool_size(&self, n: usize) -> Result<usize, ThreadPoolError> {
        if n < 1 {
            return Err(ThreadPoolError::ValueError);
        }
        self.desired_threads.store(n, Ordering::Relaxed);

        match self.control_tx.send(UPDATE_SIZE) {
            Ok(_) => Ok(n),
            Err(_) => Err(ThreadPoolError::ThreadsLost),
        }
    }

    /// Returns the latest metric values, which is updated approx. every 10 seconds.
    /// Calling this function between updates, will return the same values.
    pub fn get_metrics(&self) -> Metrics {
        self.metrics_rx.recv().unwrap()
    }
}

impl fmt::Display for ThreadPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        "Lambda Channel Thread Pool".fmt(f)
    }
}

fn spawn_primary_lambda_thread<T: Send + 'static, U: Send + 'static, V: Clone + Send + 'static>(
    input_channel: Receiver<T>,
    output_channel: Sender<U>,
    shared_resource: V,
    function: fn(&V, T) -> U,
    desired_threads: Arc<AtomicUsize>,
) -> (crossbeam_channel::Sender<u8>, genzero::Receiver<Metrics>) {
    let (control_tx, control_rx) = bounded(0);
    let (mut metrics_tx, metrics_rx) = genzero::new(Metrics::default());

    spawn(move || {
        let mut threads = Vec::new();
        let ticker = tick(Duration::from_secs(10));
        let execution_metrics = Arc::new(ExecutionMetrics::new());
        let input_channel_capacity = input_channel.capacity();
        let output_channel_capacity = output_channel.capacity();

        'main: loop {
            select_biased! {
                recv(output_channel.liveness_check) -> _ => {
                    break 'main;
                },
                recv(control_rx) -> c => {
                    let command = match c {
                        Ok(v) => v,
                        Err(_) => {
                            break 'main;
                        }
                    };

                    match command {
                        UPDATE_SIZE => {
                            let target = desired_threads.load(Ordering::Relaxed);
                            let current = threads.len() + 1;

                            if current < target {
                                for _ in 0..target-current {
                                    threads.push(spawn_worker_lambda_thread(
                                        input_channel.clone(),
                                        output_channel.clone(),
                                        shared_resource.clone(),
                                        function,
                                        execution_metrics.clone(),
                                    ));
                                }
                            } else {
                                for _ in 0..current-target {
                                    let (control_tx, _) = threads.pop().unwrap();
                                    let _ = control_tx.send(STOP_THREAD);
                                }
                            }
                        },
                        STOP_THREAD => {
                            break 'main;
                        },
                        _ => {}
                    }
                },
                recv(ticker) -> _ => {
                    let thread_count = threads.len();
                    threads.retain(|thread| {
                        let delete = thread.1.is_finished();
                        !delete
                    });
                    let failed_threads = thread_count - threads.len();

                    for _ in 0..failed_threads {
                        threads.push(spawn_worker_lambda_thread(
                            input_channel.clone(),
                            output_channel.clone(),
                            shared_resource.clone(),
                            function,
                            execution_metrics.clone(),
                        ));
                    }

                    let execution_count = execution_metrics.get_and_reset_execution_count();
                    let average_execution_duration_ns = execution_metrics.get_and_reset_total_execution_time_ns() / execution_count;
                    metrics_tx.send(Metrics{
                        active_threads: threads.len(),
                        input_channel_len: input_channel.len(),
                        input_channel_capacity,
                        output_channel_len: output_channel.len(),
                        output_channel_capacity,

                        execution_count,
                        average_execution_duration_ns,
                        // minimum_execution_duration_ns: 0,
                        // maximum_execution_duration_ns: 0,
                    });
                },
                recv(input_channel.receiver) -> msg => {
                    let input = match msg {
                        Ok(v) => v,
                        Err(_) => {
                            break 'main;
                        }
                    };

                    let start_time = execution_metrics.clock.now();
                    let output = function(&shared_resource, input);
                    let execution_time = start_time.elapsed().as_nanos() as usize;

                    'inner: loop {
                        select! {
                            recv(control_rx) -> c => {
                                let command = match c {
                                    Ok(v) => v,
                                    Err(_) => {
                                        break 'main;
                                    }
                                };

                                match command {
                                    UPDATE_SIZE => {
                                        let target = desired_threads.load(Ordering::Relaxed);
                                        let current = threads.len() + 1;

                                        if current < target {
                                            for _ in 0..target-current {
                                                threads.push(spawn_worker_lambda_thread(
                                                    input_channel.clone(),
                                                    output_channel.clone(),
                                                    shared_resource.clone(),
                                                    function,
                                                    execution_metrics.clone(),
                                                ));
                                            }
                                        } else {
                                            for _ in 0..current-target {
                                                let (control_tx, _) = threads.pop().unwrap();
                                                let _ = control_tx.send(STOP_THREAD);
                                            }
                                        }
                                    },
                                    STOP_THREAD => {
                                        break 'main;
                                    },
                                    _ => {}
                                }
                            },
                            recv(ticker) -> _ => {
                                let thread_count = threads.len();
                                threads.retain(|thread| {
                                    let delete = thread.1.is_finished();
                                    !delete
                                });
                                let failed_threads = thread_count - threads.len();

                                for _ in 0..failed_threads {
                                    threads.push(spawn_worker_lambda_thread(
                                        input_channel.clone(),
                                        output_channel.clone(),
                                        shared_resource.clone(),
                                        function,
                                        execution_metrics.clone(),
                                    ));
                                }

                                let execution_count = execution_metrics.get_and_reset_execution_count();
                                let average_execution_duration_ns = execution_metrics.get_and_reset_total_execution_time_ns() / execution_count;
                                metrics_tx.send(Metrics{
                                    active_threads: threads.len(),
                                    input_channel_len: input_channel.len(),
                                    input_channel_capacity,
                                    output_channel_len: output_channel.len(),
                                    output_channel_capacity,

                                    execution_count,
                                    average_execution_duration_ns,
                                    // minimum_execution_duration_ns: 0,
                                    // maximum_execution_duration_ns: 0,
                                });
                            },
                            send(output_channel.sender, output) -> result => {
                                match result {
                                    Ok(_) => {
                                        execution_metrics.update(execution_time);
                                        break 'inner;
                                    }
                                    Err(_) => {
                                        break 'main;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    (control_tx, metrics_rx)
}

fn spawn_worker_lambda_thread<T: Send + 'static, U: Send + 'static, V: Clone + Send + 'static>(
    input_channel: Receiver<T>,
    output_channel: Sender<U>,
    shared_resource: V,
    function: fn(&V, T) -> U,
    execution_metrics: Arc<ExecutionMetrics>,
) -> (crossbeam_channel::Sender<u8>, JoinHandle<()>) {
    let (control_tx, control_rx) = bounded(0);

    let handle = spawn(move || 'main: loop {
        select_biased! {
            recv(output_channel.liveness_check) -> _ => {
                break 'main;
            },
            recv(control_rx) -> c => {
                let command = match c {
                    Ok(v) => v,
                    Err(_) => {
                        break 'main;
                    }
                };

                if command == STOP_THREAD {
                    break 'main;
                }
            },
            recv(input_channel.receiver) -> msg => {
                let input = match msg {
                    Ok(v) => v,
                    Err(_) => {
                        break 'main;
                    }
                };

                let start_time = execution_metrics.clock.now();
                let output = function(&shared_resource, input);
                let execution_time = start_time.elapsed().as_nanos() as usize;

                'inner: loop {
                    select! {
                        recv(control_rx) -> c => {
                            let command = match c {
                                Ok(v) => v,
                                Err(_) => {
                                    drop(input_channel);
                                    let _ = output_channel.send(output);
                                    break 'main;
                                }
                            };

                            if command == STOP_THREAD {
                                drop(input_channel);
                                let _ = output_channel.send(output);
                                break 'main;
                            }
                        },
                        send(output_channel.sender, output) -> result => {
                            match result {
                                Ok(_) => {
                                    execution_metrics.update(execution_time);
                                    break 'inner;
                                }
                                Err(_) => {
                                    break 'main;
                                }
                            }
                        }
                    }
                }
            }
        }
    });
    (control_tx, handle)
}

fn spawn_primary_sink_thread<T: Send + 'static, V: Clone + Send + 'static>(
    input_channel: Receiver<T>,
    shared_resource: V,
    function: fn(&V, T),
    desired_threads: Arc<AtomicUsize>,
) -> (crossbeam_channel::Sender<u8>, genzero::Receiver<Metrics>) {
    let (control_tx, control_rx) = bounded(0);
    let (mut metrics_tx, metrics_rx) = genzero::new(Metrics::default());

    spawn(move || {
        let mut threads = Vec::new();
        let ticker = tick(Duration::from_secs(10));
        let execution_metrics = Arc::new(ExecutionMetrics::new());
        let input_channel_capacity = input_channel.capacity();
        let output_channel_capacity = None;

        'main: loop {
            select_biased! {
                recv(control_rx) -> c => {
                    let command = match c {
                        Ok(v) => v,
                        Err(_) => {
                            break 'main;
                        }
                    };

                    match command {
                        UPDATE_SIZE => {
                            let target = desired_threads.load(Ordering::Relaxed);
                            let current = threads.len() + 1;

                            if current < target {
                                for _ in 0..target-current {
                                    threads.push(spawn_worker_sink_thread(
                                        input_channel.clone(),
                                        shared_resource.clone(),
                                        function,
                                        execution_metrics.clone(),
                                    ));
                                }
                            } else {
                                for _ in 0..current-target {
                                    let (control_tx, _) = threads.pop().unwrap();
                                    let _ = control_tx.send(STOP_THREAD);
                                }
                            }
                        },
                        STOP_THREAD => {
                            break 'main;
                        },
                        _ => {}
                    }
                },
                recv(ticker) -> _ => {
                    let thread_count = threads.len();
                    threads.retain(|thread| {
                        let delete = thread.1.is_finished();
                        !delete
                    });
                    let failed_threads = thread_count - threads.len();

                    for _ in 0..failed_threads {
                        threads.push(spawn_worker_sink_thread(
                            input_channel.clone(),
                            shared_resource.clone(),
                            function,
                            execution_metrics.clone(),
                        ));
                    }

                    let execution_count = execution_metrics.get_and_reset_execution_count();
                    let average_execution_duration_ns = execution_metrics.get_and_reset_total_execution_time_ns() / execution_count;
                    metrics_tx.send(Metrics{
                        active_threads: threads.len(),
                        input_channel_len: input_channel.len(),
                        input_channel_capacity,
                        output_channel_len: 0,
                        output_channel_capacity,

                        execution_count,
                        average_execution_duration_ns,
                        // minimum_execution_duration_ns: 0,
                        // maximum_execution_duration_ns: 0,
                    });
                },
                recv(input_channel.receiver) -> msg => {
                    let input = match msg {
                        Ok(v) => v,
                        Err(_) => {
                            break 'main;
                        }
                    };

                    let start_time = execution_metrics.clock.now();
                    function(&shared_resource, input);
                    let execution_time = start_time.elapsed().as_nanos() as usize;
                    execution_metrics.update(execution_time);
                }
            }
        }
    });

    (control_tx, metrics_rx)
}

fn spawn_worker_sink_thread<T: Send + 'static, V: Clone + Send + 'static>(
    input_channel: Receiver<T>,
    shared_resource: V,
    function: fn(&V, T),
    execution_metrics: Arc<ExecutionMetrics>,
) -> (crossbeam_channel::Sender<u8>, JoinHandle<()>) {
    let (control_tx, control_rx) = bounded(0);

    let handle = spawn(move || 'main: loop {
        select_biased! {
            recv(control_rx) -> c => {
                let command = match c {
                    Ok(v) => v,
                    Err(_) => {
                        break 'main;
                    }
                };

                if command == STOP_THREAD {
                    break 'main;
                }
            },
            recv(input_channel.receiver) -> msg => {
                let input = match msg {
                    Ok(v) => v,
                    Err(_) => {
                        break 'main;
                    }
                };

                let start_time = execution_metrics.clock.now();
                function(&shared_resource, input);
                let execution_time = start_time.elapsed().as_nanos() as usize;
                execution_metrics.update(execution_time);
            }
        }
    });
    (control_tx, handle)
}

#[cfg(test)]
mod tests {
    use crate::new_lambda_channel;

    use super::*;
    use std::thread::sleep;

    fn simple_task(_: &Option<()>, x: u32) -> f32 {
        x as f32
    }

    fn io_task(_: &Option<()>, x: u32) -> f32 {
        sleep(Duration::from_millis(10));
        (x as f32) / 3.0
    }

    #[test]
    fn single_worker() {
        let tasks = 100usize;
        let capacity = 10;
        let (tx, rx, _pool) = new_lambda_channel(Some(capacity), Some(capacity), None, simple_task);

        spawn(move || {
            for i in 0..tasks {
                tx.send(i as u32).unwrap();
            }
        });

        let mut c = 0usize;
        while rx.recv().is_ok() {
            c += 1;
        }

        assert_eq!(c, tasks);
    }

    #[test]
    fn many_workers() {
        let tasks = 100usize;
        let capacity = 10;
        let (tx, rx, pool) = new_lambda_channel(Some(capacity), Some(capacity), None, io_task);
        assert_eq!(pool.set_pool_size(4), Ok(4));

        let clock = Clock::new();
        let start = clock.now();
        spawn(move || {
            for i in 0..tasks {
                tx.send(i as u32).unwrap();
            }
        });

        let mut c = 0usize;
        while rx.recv().is_ok() {
            c += 1;
        }

        assert!(start.elapsed() < Duration::from_millis(4 * (tasks as u64)));
        assert_eq!(c, tasks);
    }

    #[test]
    fn drop_input_tx() {
        let capacity = 10;
        let (tx, rx, pool) = new_lambda_channel(Some(capacity), Some(capacity), None, simple_task);
        assert_eq!(pool.set_pool_size(4), Ok(4));

        for i in 0..(2 * capacity) {
            tx.send(i as u32).unwrap();
        }

        sleep(Duration::from_millis(1));

        // The 4 members are holding 4 values.
        assert_eq!(tx.len(), 6);
        assert!(rx.is_full());

        // Test recruit while blocked.
        assert_eq!(pool.set_pool_size(6), Ok(6));

        sleep(Duration::from_millis(1));

        // The 6 members are holding 6 values.
        assert_eq!(tx.len(), 4);
        assert!(rx.is_full());

        drop(tx);

        let mut c = 0usize;
        while rx.recv().is_ok() {
            c += 1;
        }

        assert_eq!(c, 2 * capacity);
    }

    #[test]
    fn drop_output_rx() {
        let capacity = 10;
        let (tx, rx, pool) = new_lambda_channel(Some(capacity), Some(capacity), None, simple_task);
        assert_eq!(pool.set_pool_size(4), Ok(4));
        drop(rx);

        let mut c = 0;
        while tx.send(0).is_ok() {
            c += 1;
        }

        assert_eq!(c, 0);
        assert_eq!(tx.len(), c);
    }

    #[test]
    fn thrash_pool_size() {
        let tasks = 100usize;
        let capacity = 10;
        let (tx, rx, pool) = new_lambda_channel(Some(capacity), Some(capacity), None, simple_task);
        assert_eq!(pool.set_pool_size(4), Ok(4));

        spawn(move || {
            for i in 0..tasks {
                tx.send(i as u32).unwrap();
            }
        });

        let mut c = 0;
        while rx.recv().is_ok() {
            c += 1;
            if c >= 10 {
                break;
            }
        }

        assert_eq!(pool.set_pool_size(6), Ok(6));
        while rx.recv().is_ok() {
            c += 1;
            if c >= 20 {
                break;
            }
        }
        assert_eq!(pool.set_pool_size(3), Ok(3));
        while rx.recv().is_ok() {
            c += 1;
            if c >= 30 {
                break;
            }
        }
        assert_eq!(pool.set_pool_size(5), Ok(5));
        sleep(Duration::from_millis(10));
        assert_eq!(pool.set_pool_size(2), Ok(2));
        while rx.recv().is_ok() {
            c += 1;
            if c >= 50 {
                break;
            }
        }
        assert_eq!(pool.set_pool_size(1), Ok(1));

        while rx.recv().is_ok() {
            c += 1;
        }

        assert_eq!(c, tasks);
    }
}
