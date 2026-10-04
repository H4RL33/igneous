//! Sorted, non-overlapping byte ranges, used to track which text currently
//! carries a tag so only the differences are applied after each change.

use std::ops::Range;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RangeSet(Vec<Range<usize>>);

impl RangeSet {
    pub fn from_ranges(ranges: impl IntoIterator<Item = Range<usize>>) -> Self {
        let mut v: Vec<_> = ranges.into_iter().filter(|r| r.start < r.end).collect();
        v.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(v.len());
        for r in v {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        Self(merged)
    }

    pub fn ranges(&self) -> &[Range<usize>] {
        &self.0
    }

    /// Parts of `self` not covered by `other`.
    pub fn difference(&self, other: &RangeSet) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut j = 0;
        for r in &self.0 {
            let mut start = r.start;
            while j < other.0.len() && other.0[j].end <= start {
                j += 1;
            }
            let mut k = j;
            while start < r.end {
                match other.0.get(k) {
                    Some(o) if o.start < r.end => {
                        if o.start > start {
                            out.push(start..o.start);
                        }
                        start = start.max(o.end);
                        k += 1;
                    }
                    _ => {
                        out.push(start..r.end);
                        break;
                    }
                }
            }
        }
        out
    }

    /// Mirrors a text edit: `deleted` bytes at `pos` replaced by `inserted`
    /// bytes. The inserted text is not covered.
    pub fn apply_edit(&mut self, pos: usize, deleted: usize, inserted: usize) {
        let del_end = pos + deleted;
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for r in self.0.drain(..) {
            if r.end <= pos {
                out.push(r);
            } else if r.start >= del_end {
                out.push(r.start - deleted + inserted..r.end - deleted + inserted);
            } else {
                if r.start < pos {
                    out.push(r.start..pos);
                }
                if r.end > del_end {
                    out.push(pos + inserted..r.end - deleted + inserted);
                }
            }
        }
        *self = Self::from_ranges(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_and_differs() {
        let a = RangeSet::from_ranges([5..8, 0..3, 2..4, 10..12]);
        assert_eq!(a.ranges(), &[0..4, 5..8, 10..12]);
        let b = RangeSet::from_ranges([1..2, 6..11]);
        assert_eq!(a.difference(&b), vec![0..1, 2..4, 5..6, 11..12]);
        assert_eq!(b.difference(&a), vec![8..10]);
        assert_eq!(a.difference(&RangeSet::default()), a.ranges().to_vec());
    }

    #[test]
    fn edits_shift_and_split() {
        let mut s = RangeSet::from_ranges([0..4, 10..20]);
        s.apply_edit(12, 0, 3); // insert inside second range
        assert_eq!(s.ranges(), &[0..4, 10..12, 15..23]);
        s.apply_edit(2, 10, 0); // delete across ranges
        assert_eq!(s.ranges(), &[0..2, 5..13]);
        s.apply_edit(0, 0, 1); // insert at start
        assert_eq!(s.ranges(), &[1..3, 6..14]);
    }
}
