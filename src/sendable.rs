use std::thread;

pub struct Sendable<T> {
    data: T,
    thread: thread::ThreadId,
}

unsafe impl<T> Send for Sendable<T> {}
unsafe impl<T> Sync for Sendable<T> {}

impl<T> Sendable<T> {
    pub fn new(data: T) -> Sendable<T> {
        Sendable {data: data, thread: thread::current().id()}
    }

    pub fn get(&self) -> Option<&T> {
        if thread::current().id() == self.thread {
            Some(&self.data)
        }else{
            None
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        if thread::current().id() == self.thread {
            Some(&mut self.data)
        }else{
            None
        }
    }
}

impl<T> Drop for Sendable<T> {
    fn drop(&mut self) {
        if thread::current().id() != self.thread {
            panic!("Unsafe drop from a different thead.");
        }
    }
}