//! Lock-free handoff from the audio callback to the UI.
//!
//! The callback only stores atomics. It never locks, allocates, or waits.
//! The UI copies a snapshot out with a sequence counter, the same idea as a
//! seqlock, and gives up after a few torn reads rather than spinning.

use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering};

use super::{HOP, WINDOW};

/// One published analysis window.
pub struct VizSnapshot {
    /// Interleaved stereo, `WINDOW` frames, from the final mix.
    pub stereo: [i16; WINDOW * 2],
    /// Peak `|sample × volume|` per Amiga channel over the window. Muted
    /// channels and volume 0 are 0. The scale is [`crate::player::CHANNEL_PEAK_SCALE`].
    pub peaks: [u16; 4],
    /// Output rate of this window. `0` means the slot was never published.
    pub rate: u32,
    /// Increments on every successful publish, starting at 1.
    pub gen: u64,
}

/// Shared slot the callback publishes into.
pub struct VizBus {
    seq: AtomicU32,
    gen: AtomicU64,
    rate: AtomicU32,
    peaks: [AtomicU32; 4],
    samples: Box<[AtomicU16]>,
}

impl VizBus {
    /// An empty slot. [`Self::load`] is `None` until the first publish.
    pub fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
            gen: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            samples: (0..WINDOW * 2).map(|_| AtomicU16::new(0)).collect(),
        }
    }

    /// Replace the latest window. Returns immediately.
    pub fn publish(&self, interleaved: &[i16], peaks: [u16; 4], rate: u32) {
        let frames = (interleaved.len() / 2).min(WINDOW);
        // Odd sequence: a reader that samples now will retry.
        self.seq.fetch_add(1, Ordering::Release);
        for index in 0..frames {
            self.samples[index * 2].store(interleaved[index * 2] as u16, Ordering::Relaxed);
            self.samples[index * 2 + 1].store(interleaved[index * 2 + 1] as u16, Ordering::Relaxed);
        }
        for index in frames..WINDOW {
            self.samples[index * 2].store(0, Ordering::Relaxed);
            self.samples[index * 2 + 1].store(0, Ordering::Relaxed);
        }
        for (slot, peak) in self.peaks.iter().zip(peaks) {
            slot.store(u32::from(peak), Ordering::Relaxed);
        }
        self.rate.store(rate, Ordering::Relaxed);
        self.gen.fetch_add(1, Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Release);
    }

    /// Copy the latest stable window. `None` if a publish is in progress
    /// for every attempt, or if nothing has been published yet.
    pub fn load(&self) -> Option<VizSnapshot> {
        for _ in 0..4 {
            let start = self.seq.load(Ordering::Acquire);
            if start & 1 != 0 {
                continue;
            }
            let mut stereo = [0i16; WINDOW * 2];
            for (dst, src) in stereo.iter_mut().zip(self.samples.iter()) {
                *dst = src.load(Ordering::Relaxed) as i16;
            }
            let mut peaks = [0u16; 4];
            for (dst, src) in peaks.iter_mut().zip(self.peaks.iter()) {
                *dst = src.load(Ordering::Relaxed).min(u32::from(u16::MAX)) as u16;
            }
            let rate = self.rate.load(Ordering::Relaxed);
            let gen = self.gen.load(Ordering::Relaxed);
            std::sync::atomic::fence(Ordering::Acquire);
            let end = self.seq.load(Ordering::Acquire);
            if start == end {
                if gen == 0 || rate == 0 {
                    return None;
                }
                return Some(VizSnapshot {
                    stereo,
                    peaks,
                    rate,
                    gen,
                });
            }
        }
        None
    }
}

impl Default for VizBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Callback-local accumulator. It is not shared, so the audio thread never
/// takes a lock to fill it.
///
/// Samples land in a ring of [`WINDOW`] frames. Once the ring is full, every
/// [`HOP`] new frames publishes the whole ring, oldest sample first. The
/// scratch buffers are allocated in [`Self::new`], so the callback does not
/// allocate.
pub(crate) struct VizAccum {
    ring: Box<[i16]>,
    /// Frame index of the next write. When the ring is full this is also the
    /// oldest frame.
    cursor: usize,
    filled: usize,
    since_publish: usize,
    peaks: [u16; 4],
    linear: Box<[i16]>,
}

impl VizAccum {
    pub(crate) fn new() -> Self {
        Self {
            ring: vec![0; WINDOW * 2].into_boxed_slice(),
            cursor: 0,
            filled: 0,
            since_publish: 0,
            peaks: [0; 4],
            linear: vec![0; WINDOW * 2].into_boxed_slice(),
        }
    }

    /// Fold `interleaved` into the ring and publish each overlapped window.
    ///
    /// `peaks` is the max over this callback. A window that completes here
    /// reports the max of every callback that contributed since the previous
    /// publish.
    pub(crate) fn push(&mut self, interleaved: &[i16], peaks: [u16; 4], bus: &VizBus, rate: u32) {
        if rate == 0 {
            return;
        }
        for (slot, peak) in self.peaks.iter_mut().zip(peaks) {
            *slot = (*slot).max(peak);
        }
        let total = interleaved.len() / 2;
        for frame in 0..total {
            let dst = self.cursor * 2;
            let src = frame * 2;
            if src + 1 >= interleaved.len() {
                break;
            }
            self.ring[dst] = interleaved[src];
            self.ring[dst + 1] = interleaved[src + 1];
            self.cursor = (self.cursor + 1) % WINDOW;
            self.filled = (self.filled + 1).min(WINDOW);
            self.since_publish += 1;
            if self.filled == WINDOW && self.since_publish >= HOP {
                self.publish(bus, rate);
            }
        }
    }

    fn publish(&mut self, bus: &VizBus, rate: u32) {
        let oldest = if self.filled == WINDOW {
            self.cursor
        } else {
            0
        };
        for index in 0..WINDOW {
            let src = if index < self.filled {
                (oldest + index) % WINDOW
            } else {
                // Unfilled rings are not published. Keep the slot silent.
                index
            };
            self.linear[index * 2] = self.ring[src * 2];
            self.linear[index * 2 + 1] = self.ring[src * 2 + 1];
        }
        bus.publish(&self.linear, self.peaks, rate);
        self.since_publish = 0;
        self.peaks = [0; 4];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_full_window_is_visible_and_a_partial_one_waits() {
        let bus = VizBus::new();
        assert!(bus.load().is_none());
        let mut accum = VizAccum::new();
        let mut chunk = vec![0i16; 200 * 2];
        chunk[0] = 1000;
        chunk[1] = -1000;
        accum.push(&chunk, [10, 0, 0, 0], &bus, 44_100);
        assert!(bus.load().is_none(), "200 frames is not a window");

        let rest = vec![7i16; (WINDOW - 200) * 2];
        accum.push(&rest, [4, 20, 0, 1], &bus, 44_100);
        let snap = bus.load().expect("published");
        assert_eq!(snap.gen, 1);
        assert_eq!(snap.rate, 44_100);
        assert_eq!(snap.stereo[0], 1000);
        assert_eq!(snap.stereo[1], -1000);
        assert_eq!(snap.stereo[400], 7);
        assert_eq!(snap.peaks, [10, 20, 0, 1]);
    }

    #[test]
    fn a_hop_keeps_the_previous_window_and_appends_the_new_samples() {
        let bus = VizBus::new();
        let mut accum = VizAccum::new();
        let mut first = vec![1i16; WINDOW * 2];
        first[0] = 111;
        accum.push(&first, [1, 0, 0, 0], &bus, 44_100);
        let snap = bus.load().expect("first window");
        assert_eq!(snap.gen, 1);
        assert_eq!(snap.stereo[0], 111);
        assert_eq!(snap.stereo[HOP * 2], 1);

        let mut hop = vec![3i16; HOP * 2];
        hop[0] = 333;
        accum.push(&hop, [9, 0, 0, 0], &bus, 44_100);
        let snap = bus.load().expect("overlapped window");
        assert_eq!(snap.gen, 2);
        // The oldest hop was overwritten, so the new oldest frame is the one
        // that used to sit `HOP` frames in.
        assert_eq!(snap.stereo[0], 1);
        assert_eq!(snap.stereo[(WINDOW - HOP) * 2], 333);
        assert_eq!(snap.stereo[(WINDOW - 1) * 2], 3);
        assert_eq!(snap.peaks[0], 9);
    }

    #[test]
    fn publishing_does_not_block_a_concurrent_reader() {
        let bus = Arc::new(VizBus::new());
        let reader = Arc::clone(&bus);
        let handle = std::thread::spawn(move || {
            for _ in 0..2_000 {
                let _ = reader.load();
            }
        });
        let mut stereo = vec![0i16; WINDOW * 2];
        for index in 0..80u16 {
            stereo[0] = index as i16;
            bus.publish(&stereo, [index, 0, 0, 0], 48_000);
        }
        handle.join().expect("reader");
        let snap = bus.load().expect("final");
        assert_eq!(snap.gen, 80);
        assert_eq!(snap.stereo[0], 79);
        assert_eq!(snap.peaks[0], 79);
    }
}
