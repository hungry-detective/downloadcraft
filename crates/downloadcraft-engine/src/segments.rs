//! The segment map: which byte ranges are done, which are being fetched and
//! by whom, and how a free connection gets work.
//!
//! This is the heart of dynamic segmentation. A file starts as one or a few
//! segments. Whenever a connection is free it calls [`SegmentMap::claim`]:
//!
//! 1. an unowned segment (left by a failed connection, or loaded from a
//!    resume journal) is taken whole;
//! 2. otherwise the segment that will take its owner the **longest** to finish
//!    is split, and the free connection takes the far part. The owner keeps
//!    downloading; its end simply moves closer (it never reconnects).
//!
//! Splits are proportional to the measured speeds, so a slow connection keeps
//! a small part and a fast one takes the rest. Segments smaller than twice the
//! minimum split size are never split.
//!
//! Pure data structure, no I/O: everything here is unit-tested.

/// End marker for a segment whose length is not known yet.
pub(crate) const OPEN_END: u64 = u64::MAX;

/// Split points are rounded down to this boundary when there is room, so
/// writes stay aligned with disk pages.
const ALIGN: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    pub id: u64,
    /// First byte of the segment when it was created (for progress views).
    pub start: u64,
    /// Next byte to receive.
    pub pos: u64,
    /// Bytes below this are on disk (`start <= written <= pos`).
    pub written: u64,
    /// One past the last byte; [`OPEN_END`] while the size is unknown.
    pub end: u64,
    /// Worker currently fetching this segment.
    pub owner: Option<usize>,
}


/// Smallest part a crawling connection's tail is split into near the end.
const ENDGAME_MIN: u64 = 256 * 1024;

impl Segment {
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }
}

/// Work handed to a worker by [`SegmentMap::claim`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Claim {
    pub id: u64,
    pub pos: u64,
    pub end: u64,
}

#[derive(Debug)]
pub(crate) struct SegmentMap {
    segs: Vec<Segment>,
    next_id: u64,
    min_split: u64,
    total: Option<u64>,
    /// Each worker's last measured speed and the fastest any worker has
    /// reached in this run (bytes/s): what "crawling" is judged against once
    /// the fast connections have finished their parts.
    speeds: Vec<f64>,
    best_speed: f64,
}

impl SegmentMap {
    /// The latest speeds (from the monitor, once a second).
    pub fn set_speeds(&mut self, speeds: Vec<f64>, best: f64) {
        self.speeds = speeds;
        self.best_speed = self.best_speed.max(best);
    }

    /// One segment covering the whole file (or an open-ended one).
    pub fn whole(total: Option<u64>, min_split: u64) -> Self {
        let mut m = Self::empty(total, min_split);
        m.push(0, total.unwrap_or(OPEN_END), None);
        m
    }

    /// `parts` equal segments (fewer when the file is small).
    pub fn even(total: u64, parts: usize, min_split: u64) -> Self {
        let mut m = Self::empty(Some(total), min_split);
        let max_parts = (total / min_split.max(1)).max(1);
        let parts = (parts.max(1) as u64).min(max_parts);
        let chunk = total / parts;
        for i in 0..parts {
            let start = i * chunk;
            let end = if i + 1 == parts { total } else { start + chunk };
            m.push(start, end, None);
        }
        m
    }

    /// Rebuild from a resume journal: `remaining` are the unfinished
    /// `[from, to)` ranges; the gaps between them are already on disk.
    pub fn from_remaining(total: u64, remaining: &[(u64, u64)], min_split: u64) -> Self {
        let mut ranges: Vec<(u64, u64)> = remaining
            .iter()
            .map(|&(a, b)| (a.min(total), b.min(total)))
            .filter(|(a, b)| a < b)
            .collect();
        ranges.sort_unstable();
        let mut m = Self::empty(Some(total), min_split);
        let mut cursor = 0;
        for (a, b) in ranges {
            let a = a.max(cursor);
            if a >= b {
                continue;
            }
            if a > cursor {
                m.push_done(cursor, a);
            }
            m.push(a, b, None);
            cursor = b;
        }
        if cursor < total {
            m.push_done(cursor, total);
        }
        m
    }

    fn empty(total: Option<u64>, min_split: u64) -> Self {
        Self { segs: Vec::new(), next_id: 0, min_split: min_split.max(1), total, speeds: Vec::new(), best_speed: 0.0 }
    }

    fn push(&mut self, start: u64, end: u64, owner: Option<usize>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.segs.push(Segment { id, start, pos: start, written: start, end, owner });
        id
    }

    fn push_done(&mut self, start: u64, end: u64) {
        let id = self.next_id;
        self.next_id += 1;
        self.segs.push(Segment { id, start, pos: end, written: end, end, owner: None });
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Segment> {
        self.segs.iter_mut().find(|s| s.id == id)
    }

    pub fn total(&self) -> Option<u64> {
        self.total
    }

    /// Fix the size once the first response names it.
    pub fn set_total(&mut self, total: u64) {
        self.total = Some(total);
        for s in &mut self.segs {
            if s.end == OPEN_END || s.end > total {
                s.end = total.max(s.pos);
            }
        }
    }

    /// An open-ended segment whose stream ended cleanly: it is complete.
    pub fn close_open(&mut self, id: u64) {
        if let Some(s) = self.get_mut(id) {
            if s.end == OPEN_END {
                s.end = s.pos;
            }
        }
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segs
    }

    pub fn bounds(&self, id: u64) -> Option<(u64, u64)> {
        self.segs.iter().find(|s| s.id == id).map(|s| (s.pos, s.end))
    }

    /// Bytes still to fetch (0 for an open-ended map until it is closed).
    #[cfg(test)]
    pub fn remaining(&self) -> u64 {
        self.segs
            .iter()
            .filter(|s| s.end != OPEN_END)
            .map(|s| s.remaining())
            .sum()
    }

    /// Bytes on disk.
    pub fn written(&self) -> u64 {
        self.segs.iter().map(|s| s.written - s.start).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.segs.iter().all(|s| s.end != OPEN_END && s.pos >= s.end && s.written >= s.end)
    }

    /// Whether any segment still needs a worker.
    pub fn has_unowned_work(&self) -> bool {
        self.segs.iter().any(|s| s.owner.is_none() && s.remaining() > 0)
    }

    /// Number of segments a new worker could split or take right now.
    pub fn claimable(&self) -> bool {
        let sp = |i: usize| self.speeds.get(i).copied().unwrap_or(0.0);
        self.has_unowned_work()
            || self.best_split(&|_| 0.0, self.min_split, false).is_some()
            || self.best_split(&sp, ENDGAME_MIN.min(self.min_split), true).is_some()
    }

    /// Unfinished `[written, end)` ranges for the resume journal.
    pub fn journal_ranges(&self) -> Vec<(u64, u64)> {
        self.segs
            .iter()
            .filter(|s| s.written < s.end)
            .map(|s| (s.written, s.end))
            .collect()
    }

    /// Give `worker` something to do: an unowned segment, or a split of the
    /// segment that will take longest. `speed(worker)` is bytes/s (0 = not
    /// measured yet). `None` when nothing is left worth a new connection.
    pub fn claim(&mut self, worker: usize, speed: &dyn Fn(usize) -> f64) -> Option<Claim> {
        // 1. Orphaned work first, biggest first.
        if let Some(s) = self
            .segs
            .iter_mut()
            .filter(|s| s.owner.is_none() && s.remaining() > 0)
            // Biggest first; on a tie the earliest, so the file fills from
            // the start (media can be previewed sooner).
            .max_by(|a, b| a.remaining().cmp(&b.remaining()).then(b.start.cmp(&a.start)))
        {
            s.owner = Some(worker);
            // A segment rebuilt after a failure restarts at what is on disk.
            s.pos = s.written;
            return Some(Claim { id: s.id, pos: s.pos, end: s.end });
        }
        // 2. Help the slowest finisher. Near the end, when no part is big
        // enough for an ordinary split, a part held by a crawling connection
        // is still split down to ENDGAME_MIN: otherwise the whole file waits
        // for its slowest connection.
        let (id, min) = match self.best_split(speed, self.min_split, false) {
            Some(id) => (id, self.min_split),
            None => (self.best_split(speed, ENDGAME_MIN.min(self.min_split), true)?, ENDGAME_MIN.min(self.min_split)),
        };
        let avg = self.average_speed(speed).max(speed(worker)).max(self.best_speed * 0.5);
        let s = self.get_mut(id)?;
        let rem = s.remaining();
        let owner_speed = s.owner.map(speed).unwrap_or(0.0);
        // Proportional split: the newcomer is expected to run at the average
        // speed; the owner keeps the share it can finish in the same time.
        let keep = if owner_speed > 0.0 && avg > 0.0 {
            ((rem as f64) * owner_speed / (owner_speed + avg)) as u64
        } else {
            rem / 2
        };
        // A crawling owner may keep less than a usual part: what it holds
        // is what the whole file ends up waiting for.
        let crawl = owner_speed > 0.0 && avg > 0.0 && owner_speed < avg * 0.5;
        let keep_min = if crawl { ALIGN.max(64 * 1024).min(min) } else { min };
        let keep = keep.clamp(keep_min, rem - min);
        let mut mid = s.pos + keep;
        let aligned = mid - mid % ALIGN;
        if aligned >= s.pos + keep_min && s.end - aligned >= min {
            mid = aligned;
        }
        let old_end = s.end;
        s.end = mid;
        let new_id = self.push(mid, old_end, Some(worker));
        Some(Claim { id: new_id, pos: mid, end: old_end })
    }

    fn average_speed(&self, speed: &dyn Fn(usize) -> f64) -> f64 {
        let v: Vec<f64> = self
            .segs
            .iter()
            .filter_map(|s| s.owner)
            .map(speed)
            .filter(|v| *v > 0.0)
            .collect();
        if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 }
    }

    /// The owned segment with the longest expected finish time that is big
    /// enough to split.
    fn best_split(&self, speed: &dyn Fn(usize) -> f64, min: u64, only_slow: bool) -> Option<u64> {
        if self.total.is_none() {
            return None;
        }
        // Judged against the fastest speed seen too: once the fast
        // connections are done, the crawling ones are all that is left.
        let avg = self.average_speed(speed).max(self.best_speed * 0.5);
        self.segs
            .iter()
            .filter(|s| s.owner.is_some() && s.end != OPEN_END && s.remaining() >= 2 * min)
            // Endgame: only owners well below the average speed (a measured
            // crawl), so a healthy tail is not cut into needless requests.
            .filter(|s| {
                !only_slow || {
                    let v = s.owner.map(speed).unwrap_or(0.0);
                    avg > 0.0 && v > 0.0 && v < avg * 0.5
                }
            })
            .map(|s| {
                let v = s.owner.map(speed).unwrap_or(0.0);
                // Unmeasured owners are ranked at the average speed: in
                // seconds like the rest (bytes would outrank any of them).
                let v = if v > 0.0 { v } else if avg > 0.0 { avg } else { 1.0 };
                let eta = s.remaining() as f64 / v;
                (s.id, eta, s.remaining())
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)))
            .map(|(id, _, _)| id)
    }

    /// Accept up to `n` received bytes for segment `id` at its current
    /// position. Returns how many belong to it (the rest crossed a split and
    /// belongs to someone else).
    pub fn take(&mut self, id: u64, worker: usize, n: u64) -> u64 {
        match self.get_mut(id) {
            Some(s) if s.owner == Some(worker) => {
                let acc = n.min(s.remaining());
                s.pos += acc;
                acc
            }
            _ => 0,
        }
    }

    /// Record that bytes up to `upto` of segment `id` are on disk.
    pub fn mark_written(&mut self, id: u64, upto: u64) {
        if let Some(s) = self.get_mut(id) {
            s.written = s.written.max(upto.min(s.pos));
        }
    }

    /// The worker gave up the segment (error, restart or exit). Bytes that
    /// were received but not written are fetched again.
    pub fn release(&mut self, id: u64, worker: usize) {
        if let Some(s) = self.get_mut(id) {
            if s.owner == Some(worker) {
                s.owner = None;
                s.pos = s.written;
            }
        }
    }

    /// Make segment `id` (starting at 0) cover the whole file, when no other
    /// segment has anything on disk. For servers that ignore ranges: one
    /// stream then fills the file instead of every segment re-reading it.
    pub fn absorb(&mut self, id: u64) -> bool {
        let ok = self.segs.iter().all(|s| s.id == id || s.written == s.start)
            && self.segs.iter().any(|s| s.id == id && s.start == 0);
        if !ok {
            return false;
        }
        let end = self.total.unwrap_or(OPEN_END);
        self.segs.retain(|s| s.id == id);
        self.segs[0].end = end;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn none(_: usize) -> f64 {
        0.0
    }

    #[test]
    fn even_split_covers_the_file() {
        let m = SegmentMap::even(100 * MIB + 7, 8, MIB);
        let s = m.segments();
        assert_eq!(s.len(), 8);
        assert_eq!(s[0].start, 0);
        assert_eq!(s.last().unwrap().end, 100 * MIB + 7);
        for w in s.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        // Small files get fewer parts than asked for.
        assert_eq!(SegmentMap::even(3 * MIB, 8, MIB).segments().len(), 3);
        assert_eq!(SegmentMap::even(10, 8, MIB).segments().len(), 1);
    }

    #[test]
    fn free_worker_halves_the_largest_segment() {
        let mut m = SegmentMap::whole(Some(64 * MIB), MIB);
        let c0 = m.claim(0, &none).unwrap();
        assert_eq!((c0.pos, c0.end), (0, 64 * MIB));
        m.take(c0.id, 0, 4 * MIB);
        let c1 = m.claim(1, &none).unwrap();
        // Remaining 60 MiB split in half after the 4 MiB already received.
        assert_eq!(c1.pos, 34 * MIB);
        assert_eq!(c1.end, 64 * MIB);
        assert_eq!(m.bounds(c0.id), Some((4 * MIB, 34 * MIB)));
        // The owner cannot read past its new end.
        assert_eq!(m.take(c0.id, 0, 40 * MIB), 30 * MIB);
    }

    #[test]
    fn slow_owner_keeps_a_small_share() {
        let mut m = SegmentMap::whole(Some(100 * MIB), MIB);
        let c0 = m.claim(0, &none).unwrap();
        let c1 = m.claim(1, &none).unwrap();
        // Worker 0 crawls at 1 MiB/s, worker 1 runs at 9 MiB/s.
        let speed = |w: usize| if w == 0 { MIB as f64 } else { 9.0 * MIB as f64 };
        m.take(c1.id, 1, 50 * MIB); // worker 1 finished its half
        let c2 = m.claim(1, &speed).unwrap();
        assert_eq!(c2.id, 2);
        let (p0, e0) = m.bounds(c0.id).unwrap();
        // The slow owner keeps far less than half of its 50 MiB.
        assert!(e0 - p0 < 15 * MIB, "kept {}", (e0 - p0) / MIB);
        assert_eq!(c2.end, 50 * MIB);
    }

    #[test]
    fn small_segments_are_not_split() {
        let mut m = SegmentMap::whole(Some(3 * MIB / 2), MIB);
        m.claim(0, &none).unwrap();
        assert!(m.claim(1, &none).is_none());
        assert!(!m.claimable());
    }

    #[test]
    fn released_work_is_taken_back_from_the_written_offset() {
        let mut m = SegmentMap::whole(Some(10 * MIB), MIB);
        let c = m.claim(0, &none).unwrap();
        m.take(c.id, 0, 3 * MIB);
        m.mark_written(c.id, 2 * MIB);
        m.release(c.id, 0);
        assert!(m.has_unowned_work());
        let again = m.claim(5, &none).unwrap();
        assert_eq!((again.id, again.pos), (c.id, 2 * MIB));
        // A stale worker can no longer feed it.
        assert_eq!(m.take(c.id, 0, MIB), 0);
    }

    #[test]
    fn journal_round_trip() {
        let mut m = SegmentMap::even(40 * MIB, 4, MIB);
        let c = m.claim(0, &none).unwrap();
        m.take(c.id, 0, 6 * MIB);
        m.mark_written(c.id, 6 * MIB);
        let ranges = m.journal_ranges();
        assert_eq!(ranges[0], (6 * MIB, 10 * MIB));
        let r = SegmentMap::from_remaining(40 * MIB, &ranges, MIB);
        assert_eq!(r.remaining(), 34 * MIB);
        assert_eq!(r.written(), 6 * MIB);
        assert!(!r.is_complete());
    }

    #[test]
    fn open_ended_stream_never_splits() {
        let mut m = SegmentMap::whole(None, MIB);
        let c = m.claim(0, &none).unwrap();
        assert_eq!(c.end, OPEN_END);
        m.take(c.id, 0, 50 * MIB);
        assert!(m.claim(1, &none).is_none());
        m.mark_written(c.id, 50 * MIB);
        m.close_open(c.id);
        assert!(m.is_complete());
    }

    #[test]
    fn absorb_takes_over_the_file() {
        let mut m = SegmentMap::even(8 * MIB, 4, MIB);
        let c = m.claim(0, &none).unwrap();
        assert_eq!(c.pos, 0);
        assert!(m.absorb(c.id));
        assert_eq!(m.segments().len(), 1);
        assert_eq!(m.bounds(c.id), Some((0, 8 * MIB)));
        let mut n = SegmentMap::even(8 * MIB, 4, MIB);
        let a = n.claim(0, &none).unwrap();
        let b = n.claim(1, &none).unwrap();
        n.take(b.id, 1, 10);
        n.mark_written(b.id, b.pos + 10);
        assert!(!n.absorb(a.id));
    }

    #[test]
    fn total_learned_late_bounds_the_segment() {
        let mut m = SegmentMap::whole(None, MIB);
        let c = m.claim(0, &none).unwrap();
        m.set_total(20 * MIB);
        assert_eq!(m.bounds(c.id), Some((0, 20 * MIB)));
        assert!(m.claim(1, &none).is_some());
    }
}
