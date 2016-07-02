use std::sync::mpsc::{RecvError, SendError};
use std::slice;
use std::mem;
use std::os::unix::io::{RawFd, AsRawFd};
use std::marker::PhantomData;

extern crate nix;

/// The sending half of a channel
#[derive(Debug)]
pub struct Sender<T: Send> {
    fd: RawFd,
    p: PhantomData<*const T>,
}

/// The receiving half of a channel
#[derive(Debug)]
pub struct Receiver<T: Send> {
    fd: RawFd,
    p: PhantomData<*const T>,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}


/// Create a new pipe-based channel
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
    let fd = nix::unistd::pipe().unwrap();
    (
        Sender { fd: fd.1, p: PhantomData },
        Receiver { fd: fd.0, p: PhantomData },
    )
}

impl<T: Send> Sender<T> {
    /// Send data to the corresponding Receiver.
    ///
    /// # Examples
    ///
    /// Successful send:
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
    /// Unsuccessful send:
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
    /// Receive data sent by the corresponding Sender.
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
            let t = mem::uninitialized();
            let s: &mut [u8] = slice::from_raw_parts_mut(mem::transmute(&t), mem::size_of::<T>());

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

            Ok(t)
        }
    }

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

/// Iterator over data sent through the channel
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
