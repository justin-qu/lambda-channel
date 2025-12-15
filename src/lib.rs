pub mod channel;
pub mod err;
pub mod thread;

/// Creates two normal [`channels`] and connects them to a new [`ThreadPool`] to create a
/// multi-producer multi-consumer multi-threaded lambda-channel.
///
/// [`channels`]: channel
/// [`ThreadPool`]: thread::ThreadPool
///
/// # Examples
///
/// ```
/// use crossbeam_channel::RecvError;
/// use lambda_channel::new_lambda_channel;
///
/// fn fib(_: &Option<()>, n: i32) -> u64 {
///     if n <= 1 {
///         n as u64
///     } else {
///         fib(&None, n - 1) + fib(&None, n - 2)
///     }
/// }
///
/// let (s, r, _p) = new_lambda_channel(None, None, None, fib);
/// s.send(20).unwrap();
/// assert_eq!(r.recv(), Ok(6765));
///
/// s.send(10).unwrap();
/// drop(s);
/// assert_eq!(r.recv(), Ok(55));
/// assert_eq!(r.recv(), Err(RecvError));
/// ```
pub fn new_lambda_channel<T: Send + 'static, U: Send + 'static, V: Clone + Send + 'static>(
    input_capacity: Option<usize>,
    output_capacity: Option<usize>,
    shared_resource: V,
    function: fn(&V, T) -> U,
) -> (channel::Sender<T>, channel::Receiver<U>, thread::ThreadPool) {
    let (out_tx, rx) = channel::new_channel(output_capacity);
    let (tx, in_rx) = channel::new_channel_with_dependency(input_capacity, &out_tx, &rx);

    let pool = thread::ThreadPool::new_lambda_pool(in_rx, out_tx, shared_resource, function);

    (tx, rx, pool)
}

/// Connects two existing [`channels`] to a new [`ThreadPool`] to
/// create a multi-producer multi-consumer multi-threaded lambda-channel.
/// While it is not required for the input channel of the lambda-channel to have the output
/// channel as a dependency, it may lead to undesired termination behaviors.
///
/// [`channels`]: channel
/// [`ThreadPool`]: thread::ThreadPool
///
/// # Examples
///
/// ```
/// use lambda_channel::new_lambda_channel_with_input_and_output;
/// use lambda_channel::channel::{new_channel, new_channel_with_dependency};
///
/// fn convert_units(c: &f64, u: f64) -> f64 {
///     c * u
/// }
///
/// fn approx_eq(v: f64, c: f64) -> bool {
///     if (v > (c + 0.000001)) || (v < (c - 0.000001)) {
///         return false;
///     }
///     true
/// }
///
/// let (s_km, r_km) = new_channel(None);
/// let (s_mile, r_mile_to_km) = new_channel_with_dependency(None, &s_km, &r_km);
/// let (s_nautical_mile, r_nautical_mile_to_km) = new_channel_with_dependency(None, &s_km, &r_km);
///
/// let _p_mile_to_km = new_lambda_channel_with_input_and_output(r_mile_to_km, s_km.clone(), 1.60934, convert_units);
/// let _p_nautical_mile_to_km = new_lambda_channel_with_input_and_output(r_nautical_mile_to_km, s_km, 1.852, convert_units);
///
/// s_mile.send(12.0).unwrap();
/// s_nautical_mile.send(2.0).unwrap();
///
/// let msg1 = r_km.recv().unwrap();
/// let msg2 = r_km.recv().unwrap();
/// assert!(approx_eq(msg1 + msg2, 23.01608));
/// ```
pub fn new_lambda_channel_with_input_and_output<
    T: Send + 'static,
    U: Send + 'static,
    V: Clone + Send + 'static,
>(
    input_receiver: channel::Receiver<T>,
    output_sender: channel::Sender<U>,
    shared_resource: V,
    function: fn(&V, T) -> U,
) -> thread::ThreadPool {
    thread::ThreadPool::new_lambda_pool(input_receiver, output_sender, shared_resource, function)
}

/// Creates a normal [`channel`] and connects it to a new [`ThreadPool`] to create a
/// multi-producer multi-threaded lambda-sink.
///
/// [`ThreadPool`]: thread::ThreadPool
///
/// # Examples
///
/// ```
/// use lambda_channel::new_lambda_sink;
///
/// fn do_something(_: &Option<()>, n: i32) {
///     println!("Do something without output: {}", n);
/// }
///
/// let (s, _p) = new_lambda_sink(Some(0), None, do_something);
/// s.send(1).unwrap();
/// ```
pub fn new_lambda_sink<T: Send + 'static, V: Clone + Send + 'static>(
    input_capacity: Option<usize>,
    shared_resource: V,
    function: fn(&V, T),
) -> (channel::Sender<T>, thread::ThreadPool) {
    let (tx, in_rx) = channel::new_channel(input_capacity);

    let pool = thread::ThreadPool::new_sink_pool(in_rx, shared_resource, function);

    (tx, pool)
}

/// Connects an existing [`channel`] to a new [`ThreadPool`] to create a
/// multi-producer multi-threaded lambda-sink.
///
/// [`ThreadPool`]: thread::ThreadPool
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use rand::{seq::SliceRandom, thread_rng};
///
/// use lambda_channel::new_lambda_sink_with_input_from;
/// use lambda_channel::channel::new_channel;
///
/// fn generate_cipher_map() -> HashMap<char, char> {
///     let mut rng = thread_rng();
///     let mut alphanumeric: Vec<char> = Vec::new();
///     alphanumeric.extend('0'..='9');
///     alphanumeric.extend('a'..='z');
///     alphanumeric.extend('A'..='Z');
///     
///     let mut shuffled_alphanumeric = alphanumeric.clone();
///     shuffled_alphanumeric.shuffle(&mut rng);
///
///     let mut cipher_map = HashMap::new();
///     for (i, &char_val) in alphanumeric.iter().enumerate() {
///         cipher_map.insert(char_val, shuffled_alphanumeric[i]);
///     }
///     cipher_map
/// }
///
/// fn encode_text(cipher: &HashMap<char, char>, text: String) {
///     let mut encoded_text = String::new();
///     println!("Encoding:    {}", text);
///     for c in text.chars() {
///         if let Some(v) = cipher.get(&c) {
///             encoded_text.push(*v)
///         } else {
///             encoded_text.push(c)
///         }
///     }
///     println!("Cipher Text: {}", encoded_text);
/// }
///
/// let cipher_map = generate_cipher_map();
/// let (s, r) = new_channel(Some(0));
/// let _p = new_lambda_sink_with_input_from(r, cipher_map, encode_text);
/// s.send("This is a test!".to_string()).unwrap();
/// ```
pub fn new_lambda_sink_with_input_from<T: Send + 'static, V: Clone + Send + 'static>(
    input_receiver: channel::Receiver<T>,
    shared_resource: V,
    function: fn(&V, T),
) -> thread::ThreadPool {
    thread::ThreadPool::new_sink_pool(input_receiver, shared_resource, function)
}
