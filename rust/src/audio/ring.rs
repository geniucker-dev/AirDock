// SPDX-License-Identifier: MPL-2.0
//! Fixed-capacity single-producer/single-consumer PCM. Neither cursor is reset by FLUSH.
use std::{
    cell::UnsafeCell,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Copy, Default)]
pub struct Sample {
    pub value: u64,
    pub epoch: u64,
    pub pts_us: i64,
}
struct Ring {
    cells: Box<[UnsafeCell<Sample>]>,
    read: AtomicUsize,
    write: AtomicUsize,
}
// Only Producer writes an unpublished cell; only Consumer reads a published cell.
// Acquire/release cursor publication prevents concurrent access to that cell.
unsafe impl Sync for Ring {}
pub struct Producer(Arc<Ring>, usize);
pub struct Consumer(Arc<Ring>, usize);
pub fn channel(capacity: usize) -> (Producer, Consumer) {
    assert!(capacity > 0);
    let ring = Arc::new(Ring {
        cells: (0..capacity)
            .map(|_| UnsafeCell::new(Sample::default()))
            .collect(),
        read: AtomicUsize::new(0),
        write: AtomicUsize::new(0),
    });
    (Producer(ring.clone(), 0), Consumer(ring, 0))
}
impl Producer {
    pub fn free(&self) -> usize {
        self.0.cells.len()
            - self
                .0
                .write
                .load(Ordering::Relaxed)
                .wrapping_sub(self.0.read.load(Ordering::Acquire))
    }
    pub fn push(&mut self, sample: Sample) -> bool {
        let write = self.0.write.load(Ordering::Relaxed);
        if write.wrapping_sub(self.0.read.load(Ordering::Acquire)) >= self.0.cells.len() {
            return false;
        }
        unsafe {
            *self.0.cells[self.1].get() = sample;
        }
        self.1 += 1;
        if self.1 == self.0.cells.len() {
            self.1 = 0;
        }
        self.0.write.store(write.wrapping_add(1), Ordering::Release);
        true
    }
}
impl Consumer {
    pub fn pop(&mut self) -> Option<Sample> {
        let read = self.0.read.load(Ordering::Relaxed);
        if read == self.0.write.load(Ordering::Acquire) {
            return None;
        }
        let sample = unsafe { *self.0.cells[self.1].get() };
        self.1 += 1;
        if self.1 == self.0.cells.len() {
            self.1 = 0;
        }
        self.0.read.store(read.wrapping_add(1), Ordering::Release);
        Some(sample)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_ring_wraps_without_resetting_cursors() {
        let (mut p, mut c) = channel(3);
        for i in 0..100 {
            assert!(p.push(Sample {
                value: i as u64,
                epoch: 2,
                pts_us: i
            }));
            let s = c.pop().unwrap();
            assert_eq!(s.pts_us, i);
            assert_eq!(s.value, i as u64);
        }
        for _ in 0..3 {
            assert!(p.push(Sample::default()));
        }
        assert!(!p.push(Sample::default()));
        assert_eq!(p.free(), 0);
    }
    #[test]
    fn arbitrary_capacity_survives_monotonic_counter_wrap() {
        let (mut p, mut c) = channel(3);
        p.0.read.store(usize::MAX - 1, Ordering::Relaxed);
        p.0.write.store(usize::MAX - 1, Ordering::Relaxed);
        for value in 1..=3 {
            assert!(p.push(Sample {
                value,
                ..Sample::default()
            }));
        }
        assert!(!p.push(Sample::default()));
        for value in 1..=3 {
            assert_eq!(c.pop().unwrap().value, value);
        }
        assert_eq!(p.free(), 3);
        assert!(p.push(Sample {
            value: 99,
            ..Sample::default()
        }));
        assert_eq!(c.pop().unwrap().value, 99);
    }
    #[test]
    fn producer_consumer_publication_is_ordered() {
        let (mut p, mut c) = channel(31);
        let thread = std::thread::spawn(move || {
            for i in 0..50_000 {
                while !p.push(Sample {
                    value: 0,
                    epoch: 9,
                    pts_us: i,
                }) {
                    std::thread::yield_now();
                }
            }
        });
        for i in 0..50_000 {
            let s = loop {
                if let Some(s) = c.pop() {
                    break s;
                }
                std::thread::yield_now();
            };
            assert_eq!(s.epoch, 9);
            assert_eq!(s.pts_us, i);
        }
        thread.join().unwrap();
    }
}
