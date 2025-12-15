use std::fmt;
use std::time::{Duration, Instant};

use crossbeam_channel::*;

/// The sending side of a channel, almost identical to [`crossbeam_channel::Sender`]. The only difference is that
/// you can make one channel depend on another channel. If channel `A` depends on channel `B`, channel `A`
/// will **ACT** disconnected when channel `B` is disconnected. This mean that dependency is not transitive.
/// If channel `Z` depends on channel `A`, channel `Z` will not **ACT** disconnected when channel `B` is disconnected.
///
/// # Examples
///
/// Channel without dependency:
///
/// ```
/// use std::thread;
/// use crossbeam_channel::RecvError;
/// use lambda_channel::channel::new_channel;
///
/// let (s1, r) = new_channel(None);
/// let s2 = s1.clone();
///
/// thread::spawn(move || s1.send(1).unwrap());
/// thread::spawn(move || s2.send(2).unwrap());
///
/// let msg1 = r.recv().unwrap();
/// let msg2 = r.recv().unwrap();
///
/// assert_eq!(msg1 + msg2, 3);
/// assert_eq!(r.recv(), Err(RecvError)) // All senders have been dropped
/// ```
///
/// Channel with dependency:
///
/// ```
/// use std::thread;
/// use crossbeam_channel::{RecvError, SendError};
/// use lambda_channel::channel::{new_channel, new_channel_with_dependency};
///
/// let (s_b1, r_b) = new_channel(None);
/// // Channel A depends on channel B
/// let (s_a1, r_a) = new_channel_with_dependency(None, &s_b1, &r_b);
/// let s_b2 = s_b1.clone();
///
/// s_a1.send(0).unwrap();
///
/// thread::spawn(move || s_b1.send(1).unwrap());
/// thread::spawn(move || s_b2.send(2).unwrap());
///
/// let msg1 = r_b.recv().unwrap();
/// let msg2 = r_b.recv().unwrap();
///
/// assert_eq!(msg1 + msg2, 3);
/// assert_eq!(r_b.recv(), Err(RecvError)); // All `B` senders have been dropped
///
/// // Channel `B` is disconnected, channel `A` disconnects as well
/// assert_eq!(s_a1.send(3), Err(SendError(3)));
/// assert_eq!(r_a.recv(), Ok(0));
/// assert_eq!(r_a.recv(), Err(RecvError));
/// ```
pub struct Sender<T> {
    pub(super) _liveness_check: crossbeam_channel::Sender<()>,
    pub(super) sender: crossbeam_channel::Sender<T>,
    pub(super) liveness_check: crossbeam_channel::Receiver<()>,
    pub(super) depends_on: Option<(
        crossbeam_channel::Receiver<()>,
        crossbeam_channel::Receiver<()>,
    )>,
}

impl<T> Sender<T> {
    /// Blocks the current thread until a message is sent or the channel is disconnected.
    ///
    /// If the channel is full and not disconnected, this call will block until the send operation
    /// can proceed. If the channel becomes disconnected, this call will wake up and return an
    /// error. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::SendError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(Some(1));
    /// assert_eq!(s.send(1), Ok(()));
    ///
    /// thread::spawn(move || {
    ///     assert_eq!(r.recv(), Ok(1));
    ///     thread::sleep(Duration::from_secs(1));
    ///     drop(r);
    /// });
    ///
    /// assert_eq!(s.send(2), Ok(()));
    /// assert_eq!(s.send(3), Err(SendError(3)));
    /// ```
    pub fn send(&self, msg: T) -> Result<(), SendError<T>> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(dependency.0) -> _ => {
                    Err(SendError(msg))
                },
                recv(dependency.1) -> _ => {
                    Err(SendError(msg))
                },
                send(self.sender, msg) -> e => {
                    e
                }
            }
        } else {
            self.sender.send(msg)
        }
    }

    /// Attempts to send a message into the channel without blocking.
    ///
    /// This method will either send a message into the channel immediately or return an error if
    /// the channel is full or disconnected. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will send the message only if there
    /// happens to be a receive operation on the other side of the channel at the same time.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::TrySendError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(Some(1));
    ///
    /// assert_eq!(s.try_send(1), Ok(()));
    /// assert_eq!(s.try_send(2), Err(TrySendError::Full(2)));
    ///
    /// drop(r);
    /// assert_eq!(s.try_send(3), Err(TrySendError::Disconnected(3)));
    /// ```
    pub fn try_send(&self, msg: T) -> Result<(), TrySendError<T>> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(dependency.0) -> _ => {
                    Err(TrySendError::Disconnected(msg))
                },
                recv(dependency.1) -> _ => {
                    Err(TrySendError::Disconnected(msg))
                },
                default() => self.sender.try_send(msg)
            }
        } else {
            self.sender.try_send(msg)
        }
    }

    /// Waits for a message to be sent into the channel, but only for a limited time.
    ///
    /// If the channel is full and not disconnected, this call will block until the send operation
    /// can proceed or the operation times out. If the channel becomes disconnected, this call will
    /// wake up and return an error. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::SendTimeoutError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(Some(0));
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     assert_eq!(r.recv(), Ok(2));
    ///     drop(r);
    /// });
    ///
    /// assert_eq!(
    ///     s.send_timeout(1, Duration::from_millis(500)),
    ///     Err(SendTimeoutError::Timeout(1)),
    /// );
    /// assert_eq!(
    ///     s.send_timeout(2, Duration::from_secs(1)),
    ///     Ok(()),
    /// );
    /// assert_eq!(
    ///     s.send_timeout(3, Duration::from_millis(500)),
    ///     Err(SendTimeoutError::Disconnected(3)),
    /// );
    /// ```
    pub fn send_timeout(&self, msg: T, timeout: Duration) -> Result<(), SendTimeoutError<T>> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(dependency.0) -> _ => {
                    Err(SendTimeoutError::Disconnected(msg))
                },
                recv(dependency.1) -> _ => {
                    Err(SendTimeoutError::Disconnected(msg))
                },
                send(self.sender, msg) -> res => {
                    res.map_err(|e| SendTimeoutError::Disconnected(e.into_inner()))
                },
                default(timeout) => Err(SendTimeoutError::Timeout(msg))
            }
        } else {
            self.sender.send_timeout(msg, timeout)
        }
    }

    /// Waits for a message to be sent into the channel, but only until a given deadline.
    ///
    /// If the channel is full and not disconnected, this call will block until the send operation
    /// can proceed or the operation times out. If the channel becomes disconnected, this call will
    /// wake up and return an error. The returned error contains the original message.
    ///
    /// If called on a zero-capacity channel, this method will wait for a receive operation to
    /// appear on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::{Duration, Instant};
    /// use crossbeam_channel::SendTimeoutError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(Some(0));
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     assert_eq!(r.recv(), Ok(2));
    ///     drop(r);
    /// });
    ///
    /// let now = Instant::now();
    ///
    /// assert_eq!(
    ///     s.send_deadline(1, now + Duration::from_millis(500)),
    ///     Err(SendTimeoutError::Timeout(1)),
    /// );
    /// assert_eq!(
    ///     s.send_deadline(2, now + Duration::from_millis(1500)),
    ///     Ok(()),
    /// );
    /// assert_eq!(
    ///     s.send_deadline(3, now + Duration::from_millis(2000)),
    ///     Err(SendTimeoutError::Disconnected(3)),
    /// );
    /// ```
    pub fn send_deadline(&self, msg: T, deadline: Instant) -> Result<(), SendTimeoutError<T>> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(dependency.0) -> _ => {
                    Err(SendTimeoutError::Disconnected(msg))
                },
                recv(dependency.1) -> _ => {
                    Err(SendTimeoutError::Disconnected(msg))
                },
                send(self.sender, msg) -> res => {
                    res.map_err(|e| SendTimeoutError::Disconnected(e.into_inner()))
                },
                default(deadline.saturating_duration_since(Instant::now())) => Err(SendTimeoutError::Timeout(msg))
            }
        } else {
            self.sender.send_deadline(msg, deadline)
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: Zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, _r) = new_channel(None);
    /// assert!(s.is_empty());
    ///
    /// s.send(0).unwrap();
    /// assert!(!s.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.sender.is_empty()
    }

    /// Returns `true` if the channel is full.
    ///
    /// Note: Zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, _r) = new_channel(Some(1));
    ///
    /// assert!(!s.is_full());
    /// s.send(0).unwrap();
    /// assert!(s.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.sender.is_full()
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, _r) = new_channel(None);
    /// assert_eq!(s.len(), 0);
    ///
    /// s.send(1).unwrap();
    /// s.send(2).unwrap();
    /// assert_eq!(s.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        self.sender.len()
    }

    /// Returns the channel's capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, _) = new_channel::<i32>(None);
    /// assert_eq!(s.capacity(), None);
    ///
    /// let (s, _) = new_channel::<i32>(Some(5));
    /// assert_eq!(s.capacity(), Some(5));
    ///
    /// let (s, _) = new_channel::<i32>(Some(0));
    /// assert_eq!(s.capacity(), Some(0));
    /// ```
    pub fn capacity(&self) -> Option<usize> {
        self.sender.capacity()
    }

    /// Returns `true` if senders belong to the same channel.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, _) = new_channel::<usize>(None);
    ///
    /// let s2 = s.clone();
    /// assert!(s.same_channel(&s2));
    ///
    /// let (s3, _) = new_channel(None);
    /// assert!(!s.same_channel(&s3));
    /// ```
    pub fn same_channel(&self, other: &Sender<T>) -> bool {
        self.sender.same_channel(&other.sender)
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            _liveness_check: self._liveness_check.clone(),
            sender: self.sender.clone(),
            liveness_check: self.liveness_check.clone(),
            depends_on: self.depends_on.clone(),
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Lamba Channel Sender { .. }")
    }
}

/// The sending side of a channel, almost identical to [`crossbeam_channel::Receiver`]. The only difference is that
/// you can make one channel depend on another channel. If channel `A` depends on channel `B`, channel `A`
/// will **ACT** disconnected when channel `B` is disconnected. This mean that dependency is not transitive.
/// If channel `Z` depends on channel `A`, channel `Z` will not **ACT** disconnected when channel `B` is disconnected.
///
/// # Examples
///
/// Channel without dependency:
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::RecvError;
/// use lambda_channel::channel::new_channel;
///
/// let (s, r) = new_channel(None);
///
/// thread::spawn(move || {
///     let _ = s.send(1);
///     thread::sleep(Duration::from_secs(1));
///     let _ = s.send(2);
/// });
///
/// assert_eq!(r.recv(), Ok(1)); // Received immediately.
/// assert_eq!(r.recv(), Ok(2)); // Received after 1 second.
/// assert_eq!(r.recv(), Err(RecvError)); // All senders have been dropped
/// ```
///
/// Channel with dependency:
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use crossbeam_channel::{RecvError, SendError};
/// use lambda_channel::channel::{new_channel, new_channel_with_dependency};
///
/// let (s_b, r_b) = new_channel(None);
/// let (s_a, r_a) = new_channel_with_dependency(None, &s_b, &r_b);
///
/// s_a.send(0).unwrap();
///
/// thread::spawn(move || {
///     let _ = s_b.send(1);
///     thread::sleep(Duration::from_secs(1));
///     let _ = s_b.send(2);
/// });
///
/// assert_eq!(r_b.recv(), Ok(1)); // Received immediately.
/// assert_eq!(r_b.recv(), Ok(2)); // Received after 1 second.
/// assert_eq!(r_b.recv(), Err(RecvError)); // All `B` senders have been dropped
///
/// // Channel `B` is disconnected, channel `A` disconnects as well
/// assert_eq!(s_a.send(3), Err(SendError(3)));
/// assert_eq!(r_a.recv(), Ok(0));
/// assert_eq!(r_a.recv(), Err(RecvError));
/// ```
pub struct Receiver<T> {
    pub(super) _liveness_check: crossbeam_channel::Sender<()>,
    pub(super) receiver: crossbeam_channel::Receiver<T>,
    pub(super) liveness_check: crossbeam_channel::Receiver<()>,
    pub(super) depends_on: Option<(
        crossbeam_channel::Receiver<()>,
        crossbeam_channel::Receiver<()>,
    )>,
}

impl<T> Receiver<T> {
    /// Blocks the current thread until a message is received or the channel is empty and
    /// disconnected.
    ///
    /// If the channel is empty and not disconnected, this call will block until the receive
    /// operation can proceed. If the channel is empty and becomes disconnected, this call will
    /// wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a send operation to appear
    /// on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::RecvError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5).unwrap();
    ///     drop(s);
    /// });
    ///
    /// assert_eq!(r.recv(), Ok(5));
    /// assert_eq!(r.recv(), Err(RecvError));
    /// ```
    pub fn recv(&self) -> Result<T, RecvError> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(self.receiver) -> e => {
                    e
                },
                recv(dependency.0) -> _ => {
                    Err(RecvError)
                },
                recv(dependency.1) -> _ => {
                    Err(RecvError)
                },
            }
        } else {
            self.receiver.recv()
        }
    }

    /// Attempts to receive a message from the channel without blocking.
    ///
    /// This method will either receive a message from the channel immediately or return an error
    /// if the channel is empty.
    ///
    /// If called on a zero-capacity channel, this method will receive a message only if there
    /// happens to be a send operation on the other side of the channel at the same time.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_channel::TryRecvError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    /// assert_eq!(r.try_recv(), Err(TryRecvError::Empty));
    ///
    /// s.send(5).unwrap();
    /// drop(s);
    ///
    /// assert_eq!(r.try_recv(), Ok(5));
    /// assert_eq!(r.try_recv(), Err(TryRecvError::Disconnected));
    /// ```
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(dependency.0) -> _ => {
                    self.receiver.try_recv().map_err(|_| TryRecvError::Disconnected)
                },
                recv(dependency.1) -> _ => {
                    self.receiver.try_recv().map_err(|_| TryRecvError::Disconnected)
                },
                default() => self.receiver.try_recv()
            }
        } else {
            self.receiver.try_recv()
        }
    }

    /// Waits for a message to be received from the channel, but only for a limited time.
    ///
    /// If the channel is empty and not disconnected, this call will block until the receive
    /// operation can proceed or the operation times out. If the channel is empty and becomes
    /// disconnected, this call will wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a send operation to appear
    /// on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use crossbeam_channel::RecvTimeoutError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5).unwrap();
    ///     drop(s);
    /// });
    ///
    /// assert_eq!(
    ///     r.recv_timeout(Duration::from_millis(500)),
    ///     Err(RecvTimeoutError::Timeout),
    /// );
    /// assert_eq!(
    ///     r.recv_timeout(Duration::from_secs(1)),
    ///     Ok(5),
    /// );
    /// assert_eq!(
    ///     r.recv_timeout(Duration::from_secs(1)),
    ///     Err(RecvTimeoutError::Disconnected),
    /// );
    /// ```
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(self.receiver) -> res => {
                    res.map_err(|_| RecvTimeoutError::Disconnected)
                },
                recv(dependency.0) -> _ => {
                    Err(RecvTimeoutError::Disconnected)
                },
                recv(dependency.1) -> _ => {
                    Err(RecvTimeoutError::Disconnected)
                },
                default(timeout) => Err(RecvTimeoutError::Timeout),
            }
        } else {
            self.receiver.recv_timeout(timeout)
        }
    }

    /// Waits for a message to be received from the channel, but only before a given deadline.
    ///
    /// If the channel is empty and not disconnected, this call will block until the receive
    /// operation can proceed or the operation times out. If the channel is empty and becomes
    /// disconnected, this call will wake up and return an error.
    ///
    /// If called on a zero-capacity channel, this method will wait for a send operation to appear
    /// on the other side of the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::{Instant, Duration};
    /// use crossbeam_channel::RecvTimeoutError;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    ///
    /// thread::spawn(move || {
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(5).unwrap();
    ///     drop(s);
    /// });
    ///
    /// let now = Instant::now();
    ///
    /// assert_eq!(
    ///     r.recv_deadline(now + Duration::from_millis(500)),
    ///     Err(RecvTimeoutError::Timeout),
    /// );
    /// assert_eq!(
    ///     r.recv_deadline(now + Duration::from_millis(1500)),
    ///     Ok(5),
    /// );
    /// assert_eq!(
    ///     r.recv_deadline(now + Duration::from_secs(5)),
    ///     Err(RecvTimeoutError::Disconnected),
    /// );
    /// ```
    pub fn recv_deadline(&self, deadline: Instant) -> Result<T, RecvTimeoutError> {
        if let Some(dependency) = &self.depends_on {
            select_biased! {
                recv(self.receiver) -> res => {
                    res.map_err(|_| RecvTimeoutError::Disconnected)
                },
                recv(dependency.0) -> _ => {
                    Err(RecvTimeoutError::Disconnected)
                },
                recv(dependency.1) -> _ => {
                    Err(RecvTimeoutError::Disconnected)
                },
                default(deadline.saturating_duration_since(Instant::now())) => Err(RecvTimeoutError::Timeout),
            }
        } else {
            self.receiver.recv_deadline(deadline)
        }
    }

    /// Returns `true` if the channel is empty.
    ///
    /// Note: Zero-capacity channels are always empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    ///
    /// assert!(r.is_empty());
    /// s.send(0).unwrap();
    /// assert!(!r.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Returns `true` if the channel is full.
    ///
    /// Note: Zero-capacity channels are always full.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(Some(1));
    ///
    /// assert!(!r.is_full());
    /// s.send(0).unwrap();
    /// assert!(r.is_full());
    /// ```
    pub fn is_full(&self) -> bool {
        self.receiver.is_full()
    }

    /// Returns the number of messages in the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    /// assert_eq!(r.len(), 0);
    ///
    /// s.send(1).unwrap();
    /// s.send(2).unwrap();
    /// assert_eq!(r.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    /// Returns the channel's capacity.
    ///
    /// # Examples
    ///
    /// ```
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (_, r) = new_channel::<i32>(None);
    /// assert_eq!(r.capacity(), None);
    ///
    /// let (_, r) = new_channel::<i32>(Some(5));
    /// assert_eq!(r.capacity(), Some(5));
    ///
    /// let (_, r) = new_channel::<i32>(Some(0));
    /// assert_eq!(r.capacity(), Some(0));
    /// ```
    pub fn capacity(&self) -> Option<usize> {
        self.receiver.capacity()
    }

    /// A blocking iterator over messages in the channel.
    ///
    /// Each call to [`next`] blocks waiting for the next message and then returns it. However, if
    /// the channel becomes empty and disconnected, it returns [`None`] without blocking.
    ///
    /// [`next`]: Iterator::next
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel(None);
    ///
    /// thread::spawn(move || {
    ///     s.send(1).unwrap();
    ///     s.send(2).unwrap();
    ///     s.send(3).unwrap();
    ///     drop(s); // Disconnect the channel.
    /// });
    ///
    /// // Collect all messages from the channel.
    /// // Note that the call to `collect` blocks until the sender is dropped.
    /// let v: Vec<_> = r.iter().collect();
    ///
    /// assert_eq!(v, [1, 2, 3]);
    /// ```
    pub fn iter(&self) -> Iter<'_, T> {
        self.receiver.iter()
    }

    /// A non-blocking iterator over messages in the channel.
    ///
    /// Each call to [`next`] returns a message if there is one ready to be received. The iterator
    /// never blocks waiting for the next message.
    ///
    /// [`next`]: Iterator::next
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    /// use std::time::Duration;
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (s, r) = new_channel::<i32>(None);
    ///
    /// # let t =
    /// thread::spawn(move || {
    ///     s.send(1).unwrap();
    ///     thread::sleep(Duration::from_secs(1));
    ///     s.send(2).unwrap();
    ///     thread::sleep(Duration::from_secs(2));
    ///     s.send(3).unwrap();
    /// });
    ///
    /// thread::sleep(Duration::from_secs(2));
    ///
    /// // Collect all messages from the channel without blocking.
    /// // The third message hasn't been sent yet so we'll collect only the first two.
    /// let v: Vec<_> = r.try_iter().collect();
    ///
    /// assert_eq!(v, [1, 2]);
    /// # t.join().unwrap(); // join thread to avoid https://github.com/rust-lang/miri/issues/1371
    /// ```
    pub fn try_iter(&self) -> TryIter<'_, T> {
        self.receiver.try_iter()
    }

    /// Returns `true` if receivers belong to the same channel.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use lambda_channel::channel::new_channel;
    ///
    /// let (_, r) = new_channel::<usize>(None);
    ///
    /// let r2 = r.clone();
    /// assert!(r.same_channel(&r2));
    ///
    /// let (_, r3) = new_channel(None);
    /// assert!(!r.same_channel(&r3));
    /// ```
    pub fn same_channel(&self, other: &Receiver<T>) -> bool {
        self.receiver.same_channel(&other.receiver)
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        Receiver {
            _liveness_check: self._liveness_check.clone(),
            receiver: self.receiver.clone(),
            liveness_check: self.liveness_check.clone(),
            depends_on: self.depends_on.clone(),
        }
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("Lamba Channel Receiver { .. }")
    }
}

/// Creates a multi-producer multi-consumer channel of either bounded or unbounded capacity.
///
/// - If `capacity` is `None`, this channel has a growable buffer that can hold any number of messages at a time.
/// - If `capacity` is `Some(n)`, this channel has a buffer that can hold at most `n` messages at a time.
///
/// A special case is zero-capacity channel, which cannot hold any messages. Instead, send and
/// receive operations must appear at the same time in order to pair up and pass the message over.
///
/// # Examples
///
/// A channel of unbounded capacity:
///
/// ```
/// use std::thread;
/// use lambda_channel::channel::new_channel;
///
/// let (s, r) = new_channel(None);
///
/// // Computes the n-th Fibonacci number.
/// fn fib(n: i32) -> i32 {
///     if n <= 1 {
///         n
///     } else {
///         fib(n - 1) + fib(n - 2)
///     }
/// }
///
/// // Spawn an asynchronous computation.
/// thread::spawn(move || s.send(fib(20)).unwrap());
/// assert_eq!(r.recv(), Ok(6765))
/// ```
///
/// A channel of capacity 1:
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use lambda_channel::channel::new_channel;
///
/// let (s, r) = new_channel(Some(1));
///
/// // This call returns immediately because there is enough space in the channel.
/// s.send(1).unwrap();
///
/// thread::spawn(move || {
///     // This call blocks the current thread because the channel is full.
///     // It will be able to complete only after the first message is received.
///     s.send(2).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Ok(1));
/// assert_eq!(r.recv(), Ok(2));
/// ```
///
/// A zero-capacity channel:
///
/// ```
/// use std::thread;
/// use std::time::Duration;
/// use lambda_channel::channel::new_channel;
///
/// let (s, r) = new_channel(Some(0));
///
/// thread::spawn(move || {
///     // This call blocks the current thread until a receive operation appears
///     // on the other side of the channel.
///     s.send(1).unwrap();
/// });
///
/// thread::sleep(Duration::from_secs(1));
/// assert_eq!(r.recv(), Ok(1));
/// ```
pub fn new_channel<T>(capacity: Option<usize>) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = match capacity {
        None => unbounded(),
        Some(n) => bounded(n),
    };

    let (_sender_liveness_check, sender_liveness_check) = bounded(0);
    let (_receiver_liveness_check, receiver_liveness_check) = bounded(0);

    let sender = Sender {
        _liveness_check: _sender_liveness_check,
        sender,
        liveness_check: receiver_liveness_check,
        depends_on: None,
    };

    let receiver = Receiver {
        _liveness_check: _receiver_liveness_check,
        receiver,
        liveness_check: sender_liveness_check,
        depends_on: None,
    };

    (sender, receiver)
}

/// Creates a multi-producer multi-consumer channel of either bounded or unbounded capacity that **ACTS**
/// disconnected when the channel it depends on is disconnected.
///
/// - If `capacity` is `None`, this channel has a growable buffer that can hold any number of messages at a time.
/// - If `capacity` is `Some(n)`, this channel has a buffer that can hold at most `n` messages at a time.
///
/// A special case is zero-capacity channel, which cannot hold any messages. Instead, send and
/// receive operations must appear at the same time in order to pair up and pass the message over.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use crossbeam_channel::SendError;
/// use lambda_channel::channel::{new_channel, new_channel_with_dependency};
///
/// let (s_b, r_b) = new_channel(None);
/// let (s_a, r_a) = new_channel_with_dependency(None, &s_b, &r_b);
///
/// fn fib(n: i32) -> u64 {
///     if n <= 1 {
///         n as u64
///     } else {
///         fib(n - 1) + fib(n - 2)
///     }
/// }
///
/// // Spawn an asynchronous computation.
/// let handle = thread::spawn(move || {
///     while let Ok(v) = r_a.recv() {
///         s_b.send(fib(v)).unwrap();
///     }
/// });
///
/// s_a.send(20).unwrap();
/// assert_eq!(r_b.recv(), Ok(6765));
///
/// drop(r_b);
/// let _ = handle.join();
/// assert_eq!(s_a.send(10), Err(SendError(10)));
/// ```
pub fn new_channel_with_dependency<T, U>(
    capacity: Option<usize>,
    dependency_sender: &Sender<U>,
    dependency_receiver: &Receiver<U>,
) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = match capacity {
        None => unbounded(),
        Some(n) => bounded(n),
    };

    let (_sender_liveness_check, sender_liveness_check) = bounded(0);
    let (_receiver_liveness_check, receiver_liveness_check) = bounded(0);

    let sender = Sender {
        _liveness_check: _sender_liveness_check,
        sender,
        liveness_check: receiver_liveness_check,
        depends_on: Some((
            dependency_sender.liveness_check.clone(),
            dependency_receiver.liveness_check.clone(),
        )),
    };

    let receiver = Receiver {
        _liveness_check: _receiver_liveness_check,
        receiver,
        liveness_check: sender_liveness_check,
        depends_on: Some((
            dependency_sender.liveness_check.clone(),
            dependency_receiver.liveness_check.clone(),
        )),
    };

    (sender, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    use quanta::Clock;

    fn send_test(tx: Sender<u16>, rx: Receiver<u16>, handle: Option<thread::JoinHandle<()>>) {
        assert_eq!(tx.send(1), Ok(()));
        assert_eq!(rx.recv(), Ok(1));

        drop(rx);
        assert_eq!(tx.send(2), Err(SendError(2)));

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_send() {
        let (tx, rx) = new_channel(None);
        send_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_send() {
        let (out_tx, rx) = new_channel(None);
        let (tx, in_rx) = new_channel_with_dependency(None, &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        send_test(tx, rx, Some(handle));
    }

    fn try_send_test(tx: Sender<u16>, rx: Receiver<u16>, handle: Option<thread::JoinHandle<()>>) {
        assert_eq!(tx.try_send(1), Ok(()));
        assert_eq!(tx.try_send(2), Err(TrySendError::Full(2)));
        assert_eq!(rx.recv(), Ok(1));

        drop(rx);
        assert_eq!(tx.try_send(3), Err(TrySendError::Disconnected(3)));

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_try_send() {
        let (tx, rx) = new_channel(Some(1));
        try_send_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_try_send() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx, in_rx) = new_channel_with_dependency(Some(0), &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        // Make sure the thread is ready
        assert_eq!(tx.send(0), Ok(()));
        assert_eq!(rx.recv(), Ok(0));

        try_send_test(tx, rx, Some(handle));
    }

    fn send_timeout_test(
        tx: Sender<u16>,
        rx: Receiver<u16>,
        handle: Option<thread::JoinHandle<()>>,
    ) {
        let timeout = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        assert_eq!(tx.send_timeout(1, timeout), Ok(()));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(2, timeout),
            Err(SendTimeoutError::Timeout(2))
        );
        assert!(s.elapsed() >= timeout);
        assert_eq!(rx.recv(), Ok(1));

        drop(rx);
        s = clock.now();
        assert_eq!(
            tx.send_timeout(3, timeout),
            Err(SendTimeoutError::Disconnected(3))
        );
        assert!(s.elapsed() < timeout / 4);

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_send_timeout() {
        let (tx, rx) = new_channel(Some(1));
        send_timeout_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_send_timeout() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx, in_rx) = new_channel_with_dependency(Some(0), &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        send_timeout_test(tx, rx, Some(handle));
    }

    fn send_deadline_test(
        tx: Sender<u16>,
        rx: Receiver<u16>,
        handle: Option<thread::JoinHandle<()>>,
    ) {
        let timeout = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        let mut deadline = Instant::now() + timeout;
        assert_eq!(tx.send_deadline(1, deadline), Ok(()));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(2, deadline),
            Err(SendTimeoutError::Timeout(2))
        );
        assert!(s.elapsed() >= timeout);
        assert_eq!(rx.recv(), Ok(1));

        drop(rx);
        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(3, deadline),
            Err(SendTimeoutError::Disconnected(3))
        );
        assert!(s.elapsed() < timeout / 4);

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_send_deadline() {
        let (tx, rx) = new_channel(Some(1));
        send_deadline_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_send_deadline() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx, in_rx) = new_channel_with_dependency(Some(0), &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        send_deadline_test(tx, rx, Some(handle));
    }

    fn recv_test(tx: Sender<u16>, rx: Receiver<u16>, handle: Option<thread::JoinHandle<()>>) {
        assert_eq!(tx.send(1), Ok(()));
        assert_eq!(rx.recv(), Ok(1));

        assert_eq!(tx.send(2), Ok(()));
        drop(tx);

        assert_eq!(rx.recv(), Ok(2));
        assert_eq!(rx.recv(), Err(RecvError));

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_recv() {
        let (tx, rx) = new_channel(None);
        recv_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_recv() {
        let (out_tx, rx) = new_channel(None);
        let (tx, in_rx) = new_channel_with_dependency(None, &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        recv_test(tx, rx, Some(handle));
    }

    fn try_recv_test(tx: Sender<u16>, rx: Receiver<u16>, handle: Option<thread::JoinHandle<()>>) {
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));

        assert_eq!(tx.send(1), Ok(()));

        loop {
            match rx.try_recv() {
                Ok(v) => {
                    assert_eq!(v, 1);
                    break;
                }
                Err(e) => {
                    assert_eq!(e, TryRecvError::Empty);
                }
            };
        }

        assert_eq!(tx.send(2), Ok(()));
        drop(tx);

        loop {
            match rx.try_recv() {
                Ok(v) => {
                    assert_eq!(v, 2);
                    break;
                }
                Err(e) => {
                    assert_eq!(e, TryRecvError::Empty);
                }
            };
        }

        loop {
            match rx.try_recv() {
                Ok(_) => {
                    assert!(false);
                }
                Err(e) => match e {
                    TryRecvError::Empty => {}
                    TryRecvError::Disconnected => break,
                },
            };
        }

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_try_recv() {
        let (tx, rx) = new_channel(Some(1));
        try_recv_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_try_recv() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx, in_rx) = new_channel_with_dependency(Some(0), &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        try_recv_test(tx, rx, Some(handle));
    }

    fn recv_timeout_test(
        tx: Sender<u16>,
        rx: Receiver<u16>,
        handle: Option<thread::JoinHandle<()>>,
    ) {
        let timeout: Duration = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Err(RecvTimeoutError::Timeout));
        assert!(s.elapsed() >= timeout);

        assert_eq!(tx.send(1), Ok(()));
        s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Ok(1));
        assert!(s.elapsed() < timeout / 4);

        assert_eq!(tx.send(2), Ok(()));
        drop(tx);

        s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Ok(2));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            rx.recv_timeout(timeout),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_recv_timeout() {
        let (tx, rx) = new_channel(Some(1));
        recv_timeout_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_recv_timeout() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx, in_rx) = new_channel_with_dependency(Some(0), &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        recv_timeout_test(tx, rx, Some(handle));
    }

    fn recv_deadline_test(
        tx: Sender<u16>,
        rx: Receiver<u16>,
        handle: Option<thread::JoinHandle<()>>,
    ) {
        let timeout: Duration = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        let mut deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Err(RecvTimeoutError::Timeout));
        assert!(s.elapsed() >= timeout);

        assert_eq!(tx.send(1), Ok(()));
        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Ok(1));
        assert!(s.elapsed() < timeout / 4);

        assert_eq!(tx.send(2), Ok(()));
        drop(tx);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Ok(2));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            rx.recv_deadline(deadline),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);

        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    #[test]
    fn test_recv_deadline() {
        let (tx, rx) = new_channel(Some(1));
        recv_deadline_test(tx, rx, None);
    }

    #[test]
    fn test_dependent_recv_deadline() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx, in_rx) = new_channel_with_dependency(Some(0), &out_tx, &rx);

        let handle = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx.recv().unwrap());
        });

        recv_deadline_test(tx, rx, Some(handle));
    }

    #[test]
    fn test_crazy_chain_drop_receiver() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx1, in_rx1) = new_channel_with_dependency(Some(0), &out_tx, &rx);
        let (tx2, in_rx2) = new_channel_with_dependency(Some(0), &out_tx, &rx);
        let (tx11, in_rx11) = new_channel_with_dependency(Some(0), &tx1, &in_rx1);
        let (tx12, in_rx12) = new_channel_with_dependency(Some(0), &tx1, &in_rx1);
        let (tx21, in_rx21) = new_channel_with_dependency(Some(0), &tx2, &in_rx2);
        let (tx22, in_rx22) = new_channel_with_dependency(Some(0), &tx2, &in_rx2);

        let v = out_tx.clone();
        let handle1 = thread::spawn(move || loop {
            let _ = v.send(in_rx1.recv().unwrap());
        });

        let handle2 = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx2.recv().unwrap());
        });

        let v = tx1.clone();
        let handle11 = thread::spawn(move || loop {
            let _ = v.send(in_rx11.recv().unwrap());
        });

        let handle12 = thread::spawn(move || loop {
            let _ = tx1.send(in_rx12.recv().unwrap());
        });

        let v = tx2.clone();
        let handle21 = thread::spawn(move || loop {
            let _ = v.send(in_rx21.recv().unwrap());
        });

        let handle22 = thread::spawn(move || loop {
            let _ = tx2.send(in_rx22.recv().unwrap());
        });

        assert_eq!(tx11.send(1), Ok(()));
        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(tx12.send(2), Ok(()));
        assert_eq!(rx.recv(), Ok(2));
        assert_eq!(tx21.send(3), Ok(()));
        assert_eq!(rx.recv(), Ok(3));
        assert_eq!(tx22.send(4), Ok(()));
        assert_eq!(rx.recv(), Ok(4));

        drop(rx);

        let _ = handle1.join();
        let _ = handle2.join();
        let _ = handle11.join();
        let _ = handle12.join();
        let _ = handle21.join();
        let _ = handle22.join();

        assert_eq!(tx11.send(6), Err(SendError(6)));
        assert_eq!(tx12.send(7), Err(SendError(7)));
        assert_eq!(tx21.send(8), Err(SendError(8)));
        assert_eq!(tx22.send(9), Err(SendError(9)));
    }

    #[test]
    fn test_crazy_chain_drop_senders() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx1, in_rx1) = new_channel_with_dependency(Some(0), &out_tx, &rx);
        let (tx2, in_rx2) = new_channel_with_dependency(Some(0), &out_tx, &rx);
        let (tx11, in_rx11) = new_channel_with_dependency(Some(0), &tx1, &in_rx1);
        let (tx12, in_rx12) = new_channel_with_dependency(Some(0), &tx1, &in_rx1);
        let (tx21, in_rx21) = new_channel_with_dependency(Some(0), &tx2, &in_rx2);
        let (tx22, in_rx22) = new_channel_with_dependency(Some(0), &tx2, &in_rx2);

        let v = out_tx.clone();
        let handle1 = thread::spawn(move || loop {
            let _ = v.send(in_rx1.recv().unwrap());
        });

        let handle2 = thread::spawn(move || loop {
            let _ = out_tx.send(in_rx2.recv().unwrap());
        });

        let v = tx1.clone();
        let handle11 = thread::spawn(move || loop {
            let _ = v.send(in_rx11.recv().unwrap());
        });

        let handle12 = thread::spawn(move || loop {
            let _ = tx1.send(in_rx12.recv().unwrap());
        });

        let v = tx2.clone();
        let handle21 = thread::spawn(move || loop {
            let _ = v.send(in_rx21.recv().unwrap());
        });

        let handle22 = thread::spawn(move || loop {
            let _ = tx2.send(in_rx22.recv().unwrap());
        });

        assert_eq!(tx11.send(1), Ok(()));
        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(tx12.send(2), Ok(()));
        assert_eq!(rx.recv(), Ok(2));
        assert_eq!(tx21.send(3), Ok(()));
        assert_eq!(rx.recv(), Ok(3));
        assert_eq!(tx22.send(4), Ok(()));
        assert_eq!(rx.recv(), Ok(4));

        assert_eq!(tx11.send(5), Ok(()));
        drop(tx11);
        drop(tx12);
        drop(tx21);
        drop(tx22);

        assert_eq!(rx.recv(), Ok(5));
        assert_eq!(rx.recv(), Err(RecvError));

        let _ = handle1.join();
        let _ = handle2.join();
        let _ = handle11.join();
        let _ = handle12.join();
        let _ = handle21.join();
        let _ = handle22.join();
    }

    #[test]
    fn test_crazy_chain_drop_threads() {
        let (out_tx, rx) = new_channel(Some(0));
        let (tx1, in_rx1) = new_channel_with_dependency(Some(0), &out_tx, &rx);
        let (tx2, in_rx2) = new_channel_with_dependency(Some(0), &out_tx, &rx);
        let (tx11, in_rx11) = new_channel_with_dependency(Some(0), &tx1, &in_rx1);
        let (tx12, in_rx12) = new_channel_with_dependency(Some(0), &tx1, &in_rx1);
        let (tx21, in_rx21) = new_channel_with_dependency(Some(0), &tx2, &in_rx2);
        let (tx22, in_rx22) = new_channel_with_dependency(Some(0), &tx2, &in_rx2);

        let v = out_tx.clone();
        let thread1_kill = Arc::new(AtomicBool::new(true));
        let tk1 = thread1_kill.clone();
        let handle1 = thread::spawn(move || {
            while tk1.load(Ordering::Acquire) {
                let _ = v.send(in_rx1.recv().unwrap());
            }
        });

        let thread2_kill = Arc::new(AtomicBool::new(true));
        let tk2 = thread2_kill.clone();
        let handle2 = thread::spawn(move || {
            while tk2.load(Ordering::Acquire) {
                let _ = out_tx.send(in_rx2.recv().unwrap());
            }
        });

        let v = tx1.clone();
        let handle11 = thread::spawn(move || loop {
            let _ = v.send(in_rx11.recv().unwrap());
        });

        let handle12 = thread::spawn(move || loop {
            let _ = tx1.send(in_rx12.recv().unwrap());
        });

        let v = tx2.clone();
        let handle21 = thread::spawn(move || loop {
            let _ = v.send(in_rx21.recv().unwrap());
        });

        let handle22 = thread::spawn(move || loop {
            let _ = tx2.send(in_rx22.recv().unwrap());
        });

        assert_eq!(tx11.send(1), Ok(()));
        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(tx11.send(2), Ok(()));
        assert_eq!(rx.recv(), Ok(2));
        assert_eq!(tx11.send(3), Ok(()));
        assert_eq!(rx.recv(), Ok(3));
        assert_eq!(tx11.send(4), Ok(()));
        assert_eq!(rx.recv(), Ok(4));

        thread1_kill.store(false, Ordering::Release);
        assert_eq!(tx11.send(5), Ok(()));
        assert_eq!(rx.recv(), Ok(5));
        let _ = handle1.join();

        assert_eq!(tx11.send(6), Err(SendError(6)));
        assert_eq!(tx12.send(7), Err(SendError(7)));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));

        thread2_kill.store(false, Ordering::Release);
        assert_eq!(tx21.send(8), Ok(()));
        assert_eq!(rx.recv(), Ok(8));
        let _ = handle2.join();

        assert_eq!(tx21.send(9), Err(SendError(9)));
        assert_eq!(tx22.send(10), Err(SendError(10)));
        assert_eq!(rx.recv(), Err(RecvError));

        let _ = handle11.join();
        let _ = handle12.join();
        let _ = handle21.join();
        let _ = handle22.join();
    }

    #[test]
    fn test_dependency_sender_loss() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        assert_eq!(tx.send(1), Ok(()));

        drop(dep_tx);

        assert_eq!(tx.send(2), Err(SendError(2)));
        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(rx.recv(), Err(RecvError));

        drop(dep_rx);

        assert_eq!(tx.send(3), Err(SendError(3)));
        assert_eq!(rx.recv(), Err(RecvError));
    }

    #[test]
    fn test_dependency_receiver_loss() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        assert_eq!(tx.send(1), Ok(()));

        drop(dep_rx);

        assert_eq!(tx.send(2), Err(SendError(2)));
        assert_eq!(rx.recv(), Ok(1));
        assert_eq!(rx.recv(), Err(RecvError));

        drop(dep_tx);

        assert_eq!(tx.send(3), Err(SendError(3)));
        assert_eq!(rx.recv(), Err(RecvError));
    }

    #[test]
    fn test_dependency_sender_loss_try() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(tx.try_send(1), Ok(()));
        assert_eq!(tx.try_send(2), Err(TrySendError::Full(2)));

        drop(dep_tx);

        assert_eq!(tx.try_send(3), Err(TrySendError::Disconnected(3)));
        assert_eq!(rx.try_recv(), Ok(1));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));

        drop(dep_rx);

        assert_eq!(tx.try_send(4), Err(TrySendError::Disconnected(4)));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn test_dependency_receiver_loss_try() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(tx.try_send(1), Ok(()));
        assert_eq!(tx.try_send(2), Err(TrySendError::Full(2)));

        drop(dep_rx);

        assert_eq!(tx.try_send(3), Err(TrySendError::Disconnected(3)));
        assert_eq!(rx.try_recv(), Ok(1));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));

        drop(dep_tx);

        assert_eq!(tx.try_send(4), Err(TrySendError::Disconnected(4)));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn test_dependency_sender_loss_timeout() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        let timeout = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Err(RecvTimeoutError::Timeout));
        assert!(s.elapsed() >= timeout);

        s = clock.now();
        assert_eq!(tx.send_timeout(1, timeout), Ok(()));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(2, timeout),
            Err(SendTimeoutError::Timeout(2))
        );
        assert!(s.elapsed() >= timeout);

        drop(dep_tx);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(3, timeout),
            Err(SendTimeoutError::Disconnected(3))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Ok(1));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            rx.recv_timeout(timeout),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);

        drop(dep_rx);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(4, timeout),
            Err(SendTimeoutError::Disconnected(4))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            rx.recv_timeout(timeout),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);
    }

    #[test]
    fn test_dependency_receiver_loss_timeout() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        let timeout = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Err(RecvTimeoutError::Timeout));
        assert!(s.elapsed() >= timeout);

        s = clock.now();
        assert_eq!(tx.send_timeout(1, timeout), Ok(()));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(2, timeout),
            Err(SendTimeoutError::Timeout(2))
        );
        assert!(s.elapsed() >= timeout);

        drop(dep_rx);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(3, timeout),
            Err(SendTimeoutError::Disconnected(3))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(rx.recv_timeout(timeout), Ok(1));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            rx.recv_timeout(timeout),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);

        drop(dep_tx);

        s = clock.now();
        assert_eq!(
            tx.send_timeout(4, timeout),
            Err(SendTimeoutError::Disconnected(4))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        assert_eq!(
            rx.recv_timeout(timeout),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);
    }

    #[test]
    fn test_dependency_sender_loss_deadline() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        let timeout = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        let mut deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Err(RecvTimeoutError::Timeout));
        assert!(s.elapsed() >= timeout);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(tx.send_deadline(1, deadline), Ok(()));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(2, deadline),
            Err(SendTimeoutError::Timeout(2))
        );
        assert!(s.elapsed() >= timeout);

        drop(dep_tx);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(3, deadline),
            Err(SendTimeoutError::Disconnected(3))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Ok(1));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            rx.recv_deadline(deadline),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);

        drop(dep_rx);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(4, deadline),
            Err(SendTimeoutError::Disconnected(4))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            rx.recv_deadline(deadline),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);
    }

    #[test]
    fn test_dependency_receiver_loss_deadline() {
        let (dep_tx, dep_rx) = new_channel::<()>(Some(1));
        let (tx, rx) = new_channel_with_dependency(Some(1), &dep_tx, &dep_rx);

        let timeout = Duration::from_millis(10);
        let clock = Clock::new();

        let mut s = clock.now();
        let mut deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Err(RecvTimeoutError::Timeout));
        assert!(s.elapsed() >= timeout);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(tx.send_deadline(1, deadline), Ok(()));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(2, deadline),
            Err(SendTimeoutError::Timeout(2))
        );
        assert!(s.elapsed() >= timeout);

        drop(dep_rx);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(3, deadline),
            Err(SendTimeoutError::Disconnected(3))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(rx.recv_deadline(deadline), Ok(1));
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            rx.recv_deadline(deadline),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);

        drop(dep_tx);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            tx.send_deadline(4, deadline),
            Err(SendTimeoutError::Disconnected(4))
        );
        assert!(s.elapsed() < timeout / 4);

        s = clock.now();
        deadline = Instant::now() + timeout;
        assert_eq!(
            rx.recv_deadline(deadline),
            Err(RecvTimeoutError::Disconnected)
        );
        assert!(s.elapsed() < timeout / 4);
    }
}
