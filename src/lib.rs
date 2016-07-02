pub use std::sync::mpsc::{RecvError, SendError};
use std::slice;
use std::mem;
use std::os::unix::io::RawFd;
use std::marker::PhantomData;

extern crate nix;

/// The Sender part of a channel
pub struct Sender<T: Send> {
    fd: RawFd,
    p: PhantomData<*const T>,
}

/// The Receiver part of a channel
pub struct Receiver<T: Send> {
    fd: RawFd,
    p: PhantomData<*const T>,
}

unsafe impl<T: Send> Send for Sender<T> {}
unsafe impl<T: Send> Send for Receiver<T> {}


pub fn channel<T: Send>() -> (Sender<T>, Receiver<T>) {
    let fd = nix::unistd::pipe().unwrap();
    (
        Sender { fd: fd.1, p: PhantomData },
        Receiver { fd: fd.0, p: PhantomData },
    )
}

impl<T: Send> Sender<T> {
    pub fn send(&mut self, t: T) -> Result<(), SendError<T>> {
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
    pub fn recv(&mut self) -> Result<T, RecvError> {
        unsafe {
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
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
    }
}
