//! Channel implementation based on pipes.
//!
//! This crate provides a channel implementation with API similar to that of
//! [`std::sync::mpsc`](https://doc.rust-lang.org/std/sync/mpsc/index.html),
//! based on OS-level pipes. The pipes are buffered by the underlying OS kernel.
//!
//! Both [`Sender`](struct.Sender.html) and [`Receiver`](struct.Receiver.html) structs
//! implement [`AsRawFd`](https://doc.rust-lang.org/std/os/unix/io/trait.AsRawFd.html) trait,
//! making them possible to use with `select()` system call,
//! or in other places where a file descriptor is necessary.
//!
//! # Examples
//!
//! ```
//! use std::thread;
//! use pipe_channel::*;
//!
//! let (mut tx, mut rx) = channel();
//! let handle = thread::spawn(move || {
//!     tx.send(35).unwrap();
//!     tx.send(42).unwrap();
//! });
//! assert_eq!(rx.recv().unwrap(), 35);
//! assert_eq!(rx.recv().unwrap(), 42);
//! handle.join().unwrap();
//! ```
//!
//! # Ownership
//!
//! Unlike [`std::sync::mpsc`](https://doc.rust-lang.org/std/sync/mpsc/index.html) channels,
//! both `Sender::send()` and `Receiver::recv()` take `&mut self`. Thus, it's not possible
//! to share or clone `Sender`s. Use the usual `Arc<Mutex<Sender<T>>>` instead:
//!
//! ```
//! use std::thread;
//! use std::sync::{Arc, Mutex};
//! use pipe_channel::*;
//!
//! // Create a shared channel that can be sent along from many threads
//! // where tx is the sending half (tx for transmission), and rx is the receiving
//! // half (rx for receiving).
//! let (tx, mut rx) = channel();
//! let tx = Arc::new(Mutex::new(tx));
//! for i in 0..10 {
//!     let tx = tx.clone();
//!     thread::spawn(move|| {
//!         let mut tx = tx.lock().unwrap();
//!         tx.send(i).unwrap();
//!     });
//! }
//!
//! for _ in 0..10 {
//!     let j = rx.recv().unwrap();
//!     assert!(0 <= j && j < 10);
//! }
//! ```
//!
//! # Multithreading and multiprocessing
//!
//! On a lower level, it is totally supported to have pipes that go from one process to another.
//! This means that after a `fork()` it's possible to use a channel to send data between processes.
//! However, please note that the data in question may include some process-local data, like
//! references, pointers, file descriptors, etc. Thus, it's not really safe to use channels
//! this way.
//!
//! # Relation to SIGPIPE
//!
//! When the reading end has been closed, calling `write()` on a pipe sends SIGPIPE to the process.
//! This means that calling `Sender::send()` when the corresponding `Receiver` has been dropped
//! will result in SIGPIPE being sent to the process.
//!
//! It seems like SIGPIPE is ignored by default in Rust executables. Nevertheless, make sure
//! that it is in your case.
//! `Sender::send()` will only return `Err` when the underlying syscall returns `EPIPE`.
//! See the [manual page](http://linux.die.net/man/7/pipe) for more details.
//!
//! # Performance
//!
//! Unlike [`std::sync::mpsc`](https://doc.rust-lang.org/std/sync/mpsc/index.html) channels
//! that were tweaked for ultimate performance, this implementation entirely relies on the kernel.
//! Simply speaking, what it does is it copies objects bytewise in and out of pipes.
//! This should be reasonably fast for normal-sized objects. If you need to send a giant object,
//! consider wrapping it into a `Box` and sending that one instead.
//!
//! # Operating systems compatibility
//!
//! This should work on any UNIX-like OS. Being lazy, I only tested on
//! my own system (Fedora), and I'm not planning to change this; however,
//! if you test this on some other system, I'd appreciate it.
//!
//! # Panics
//!
//! The `Result`s of syscalls are `unwrap`ped (except for EPIPE). Thus, if any of them fails,
//! the program will panic. This should be rare, although not completely unexpected
//! (e.g. program can run out of file descriptors).

use std::mem;
use std::slice;
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::sync::mpsc::{RecvError, SendError};
use std::os::unix::io::{RawFd, AsRawFd};

extern crate nix;
extern crate libc;

/// The sending half of a channel.
#[derive(Debug)]
pub struct Sender<T: Send> {
    fd: RawFd,
    p: PhantomData<*const T>,
}

/// The receiving half of a channel.
#[derive(Debug)]
pub struct Receiver<T: Send> {
    fd: RawFd,
    p: PhantomData<*const T>,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}


/// Create a new pipe-based channel.
///
/// # Examples
///
/// ```
/// use std::thread;
/// use pipe_channel::*;
///
/// let (mut tx, mut rx) = channel();
/// let handle = thread::spawn(move || {
///     tx.send(35).unwrap();
///     tx.send(42).unwrap();
/// });
/// assert_eq!(rx.recv().unwrap(), 35);
/// assert_eq!(rx.recv().unwrap(), 42);
/// handle.join().unwrap();
/// ```
pub fn channel<T: Send>() -> (Sender<T>, Receiver<T>) {
    let flags = nix::fcntl::OFlag::from_bits(libc::O_CLOEXEC).unwrap();
    let fd = nix::unistd::pipe2(flags).unwrap();
    (
        Sender { fd: fd.1, p: PhantomData },
        Receiver { fd: fd.0, p: PhantomData },
    )
}

impl<T: Send> Sender<T> {
    /// Send data to the corresponding `Receiver`.
    ///
    /// This may block if the underlying syscall blocks, namely if the
    /// pipe buffer is full.
    ///
    /// # Errors
    ///
    /// If the corresponding `Receiver` is already dropped,
    /// this method will return `Err(SendError(t))`, transferring the ownership over
    /// `t` back to the caller.
    ///
    /// # Examples
    ///
    /// Success:
    ///
    /// ```
    /// use std::thread;
    /// use pipe_channel::*;
    ///
    /// let (mut tx, mut rx) = channel();
    /// let handle = thread::spawn(move || {
    ///     tx.send(35).unwrap();
    ///     tx.send(42).unwrap();
    /// });
    /// assert_eq!(rx.recv().unwrap(), 35);
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// handle.join().unwrap();
    /// ```
    ///
    /// Failure:
    ///
    /// ```
    /// use pipe_channel::*;
    /// use std::sync::mpsc::SendError;
    /// use std::mem::drop;
    ///
    /// let (mut tx, rx) = channel();
    /// drop(rx);
    /// assert_eq!(tx.send(42), Err(SendError(42)));
    /// ```
    pub fn send(&mut self, t: T) -> Result<(), SendError<T>> {
        // TODO: once constexpr is stable, change this to
        // let mut s: [u8; mem::size_of::<T>()] = mem::transmute(t);
        let s: &[u8] = unsafe {
            slice::from_raw_parts(mem::transmute(&t), mem::size_of::<T>())
        };

        let mut n = 0;
        while n < s.len() {
            match nix::unistd::write(self.fd, &s[n..]) {
                Ok(count) => n += count,
                Err(nix::Error::Sys(nix::Errno::EPIPE)) => return Err(SendError(t)),
                e => { e.unwrap(); }
            }
        }

        mem::forget(t);
        Ok(())
    }
}

impl<T: Send> Receiver<T> {
    /// Receive data sent by the corresponding `Sender`.
    ///
    /// This will block until a value is actully sent, if none is already.
    ///
    /// # Errors
    ///
    /// If the corresponding `Sender` is already dropped (or gets dropped during the wait),
    /// this method will return `Err(RecvError)`.
    ///
    /// # Examples
    ///
    /// Success:
    ///
    /// ```
    /// use std::thread;
    /// use pipe_channel::*;
    ///
    /// let (mut tx, mut rx) = channel();
    /// let handle = thread::spawn(move || {
    ///     tx.send(35).unwrap();
    ///     tx.send(42).unwrap();
    /// });
    /// assert_eq!(rx.recv().unwrap(), 35);
    /// assert_eq!(rx.recv().unwrap(), 42);
    /// handle.join().unwrap();
    /// ```
    ///
    /// Failure:
    ///
    /// ```
    /// use pipe_channel::*;
    /// use std::sync::mpsc::RecvError;
    /// use std::mem::drop;
    ///
    /// let (tx, mut rx) = channel::<i32>();
    /// drop(tx);
    /// assert_eq!(rx.recv(), Err(RecvError));
    /// ```
    pub fn recv(&mut self) -> Result<T, RecvError> {
        unsafe {
            // TODO: once constexpr is stable, change this to
            // let mut s: [u8; mem::size_of::<T>()] = mem::uninitialized();
            let t = UnsafeCell::new(mem::uninitialized());
            let s: &mut [u8] = slice::from_raw_parts_mut(t.get() as *mut u8, mem::size_of::<T>());

            let mut n = 0;
            while n < s.len() {
                match nix::unistd::read(self.fd, &mut s[n..]) {
                    Ok(0) => {
                        mem::forget(t);
                        return Err(RecvError);
                    }
                    Ok(count) => n += count,
                    e => { e.unwrap(); }
                }
            }

            Ok(t.into_inner())
        }
    }

    /// Get an iterator over data sent through the channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use pipe_channel::*;
    /// use std::mem::drop;
    ///
    /// let (mut tx, mut rx) = channel();
    /// for i in 0..1024 {
    ///     tx.send(i).unwrap();
    /// }
    /// drop(tx);
    ///
    /// for (i, j) in rx.iter().take(10).zip(0..10) {
    ///     assert_eq!(i, j);
    /// }
    /// let v1: Vec<_> = rx.into_iter().collect();
    /// let v2: Vec<_> = (10..1024).collect();
    /// assert_eq!(v1, v2);
    /// ```
    pub fn iter(&mut self) -> Iter<T> {
        self.into_iter()
    }
}

impl<T: Send> Drop for Sender<T> {
    fn drop(&mut self) {
        nix::unistd::close(self.fd).unwrap();
    }
}

impl<T: Send> Drop for Receiver<T> {
    fn drop(&mut self) {
        nix::unistd::close(self.fd).unwrap();
    }
}

impl<T: Send> AsRawFd for Sender<T> {
    fn as_raw_fd(&self) -> RawFd { self.fd }
}
impl<T: Send> AsRawFd for Receiver<T> {
    fn as_raw_fd(&self) -> RawFd { self.fd }
}

/// Iterator over data sent through the channel.
/// # Examples
///
/// ```
/// use pipe_channel::*;
/// use std::mem::drop;
///
/// let (mut tx, rx) = channel();
/// for i in 0..1024 {
///     tx.send(i);
/// }
/// drop(tx);
///
/// let v1: Vec<_> = (0..1024).collect();
/// let v2: Vec<_> = rx.into_iter().collect();
/// assert_eq!(v1, v2);
/// ```
pub struct IntoIter<T: Send>(Receiver<T>);

impl<T: Send> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.0.recv().ok()
    }
}

impl<T: Send> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter(self)
    }
}

/// Iterator over data sent through the channel.
///
/// See [`Receiver::iter()`](struct.Receiver.html#method.iter) for more information.
pub struct Iter<'a, T: 'a + Send>(&'a mut Receiver<T>);

impl<'a, T: 'a + Send> Iterator for Iter<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.0.recv().ok()
    }
}

impl<'a, T: 'a + Send> IntoIterator for &'a mut Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        Iter(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_leak() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        struct T(Arc<Mutex<i32>>);
        impl Drop for T {
            fn drop(&mut self) {
                *self.0.lock().unwrap() += 1;
            }
        }

        let cnt = Arc::new(Mutex::new(0));
        let t = T(cnt.clone());
        let (mut tx, mut rx) = channel();

        assert_eq!(*cnt.lock().unwrap(), 0);
        tx.send(t).unwrap();
        assert_eq!(*cnt.lock().unwrap(), 0);
        thread::spawn(move || rx.recv().unwrap()).join().unwrap();
        assert_eq!(*cnt.lock().unwrap(), 1);
    }

    #[test]
    fn zero_sized_type() {
        let (mut tx, mut rx) = channel();
        tx.send(()).unwrap();
        assert_eq!(rx.recv().unwrap(), ());
    }

    #[test]
    fn large_data() {
        struct Large([usize; 4096]);
        impl Large {
            fn new() -> Large {
                let mut res = [0; 4096];
                for i in 0..(res.len()) {
                    res[i] = i * i;
                }
                Large(res)
            }
        }
        unsafe impl Send for Large {};

        // may want to use threads, as it may block
        let (mut tx, mut rx) = channel();
        tx.send(Large::new()).unwrap();
        let res = rx.recv().unwrap();

        let expected = Large::new();
        for i in 0..(res.0.len()) {
            assert_eq!(res.0[i], expected.0[i]);
        }
    }
}
