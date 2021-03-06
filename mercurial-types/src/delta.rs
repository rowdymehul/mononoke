// Copyright (c) 2004-present, Facebook, Inc.
// All Rights Reserved.
//
// This software may be used and distributed according to the terms of the
// GNU General Public License version 2 or any later version.

use std::cmp;

use itertools::{self, PutBack};
use quickcheck::{Arbitrary, Gen};
use rand::distributions::{IndependentSample, LogNormal};

use errors::*;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, HeapSizeOf)]
pub struct Delta {
    // Fragments should be in sorted order by start offset and should not overlap.
    frags: Vec<Fragment>,
}

impl Delta {
    /// Construct a new Delta object. Verify that `frags` is sane, sorted and
    /// non-overlapping.
    pub fn new(frags: Vec<Fragment>) -> Result<Self> {
        Self::verify(&frags)?;
        Ok(Delta { frags: frags })
    }

    pub fn fragments(&self) -> &[Fragment] {
        self.frags.as_slice()
    }

    fn verify(frags: &[Fragment]) -> Result<()> {
        let mut prev_frag: Option<&Fragment> = None;
        for (i, frag) in frags.iter().enumerate() {
            frag.verify()
                .chain_err(|| {
                    ErrorKind::InvalidFragmentList(format!("invalid fragment {}", i))
                })?;
            if let Some(prev) = prev_frag {
                if frag.start < prev.end {
                    let msg = format!(
                        "fragment {}: previous end {} overlaps with start {}",
                        i,
                        prev.end,
                        frag.start
                    );
                    bail!(ErrorKind::InvalidFragmentList(msg));
                }
            }
            prev_frag = Some(frag);
        }
        Ok(())
    }
}

impl Default for Delta {
    fn default() -> Delta {
        Delta { frags: Vec::new() }
    }
}

impl Arbitrary for Delta {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
        let size = g.size();
        let nfrags = g.gen_range(0, size);

        // Maintain invariants (start <= end, no overlap).
        let mut start = 0;
        let mut end = 0;

        let frags = (0..nfrags)
            .map(|_| {
                start = end + g.gen_range(0, size);
                end = start + g.gen_range(0, size);
                let val = Fragment {
                    start: start,
                    end: end,
                    content: arbitrary_frag_content(g),
                };
                val
            })
            .collect();
        Delta { frags: frags }
    }

    fn shrink(&self) -> Box<Iterator<Item = Self>> {
        // Not all instances generated by Vec::shrink will be
        // valid. Theoretically we could shrink in ways such that the invariants
        // are maintained, but just filtering is easier.
        Box::new(
            self.frags
                .shrink()
                .filter(|frags| Delta::verify(&frags).is_ok())
                .map(|frags| Delta { frags: frags }),
        )
    }
}

/// Represents a single contiguous modified region of text.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, HeapSizeOf)]
pub struct Fragment {
    pub start: usize,
    pub end: usize,
    pub content: Vec<u8>,
}

impl Fragment {
    /// Return the end offset of this Fragment's content, after application.
    pub fn post_end(&self) -> usize {
        self.start + self.content.len()
    }

    /// Return the change in text length this Fragment will cause when applied.
    pub fn length_change(&self) -> isize {
        self.content.len() as isize - (self.end - self.start) as isize
    }

    /// Return true if the given offset falls within this Fragment's content (post-application).
    pub fn contains_offset(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.post_end()
    }

    /// Split the Fragment at the given offset. The Fragment is modified in-place, and the
    /// split-off portion is made into a new Fragment. Returns None if the split point
    /// does not fall within the Fragment's content bounds.
    pub fn split(&mut self, at: usize) -> Option<Fragment> {
        if !self.contains_offset(at) {
            return None;
        }
        // The split point may occur after the end index of this Fragment if the new content is
        // longer than the text being replaced. If so, clamp the split point to the end index.
        let split = cmp::min(self.end, at);

        // Adjust the original Fragment to only refer to the first part of the split content.
        let end = self.end;
        self.end = split;

        // Construct a new Fragment for the second part of the split content.
        Some(Fragment {
            start: split,
            end: end,
            content: self.content.split_off(at - self.start),
        })
    }

    fn verify(&self) -> Result<()> {
        if self.start > self.end {
            bail!("invalid fragment: start {} > end {}", self.start, self.end);
        }
        Ok(())
    }
}

impl Arbitrary for Fragment {
    fn arbitrary<G: Gen>(g: &mut G) -> Self {
        let size = g.size();

        // Maintain invariant start <= end.
        let start = g.gen_range(0, size);
        let end = start + g.gen_range(0, size);

        Fragment {
            start: start,
            end: end,
            content: arbitrary_frag_content(g),
        }
    }

    fn shrink(&self) -> Box<Iterator<Item = Self>> {
        Box::new(
            (self.start, self.end, self.content.clone())
                .shrink()
                .filter(|&(start, end, ref _content)| {
                    // shrink could produce bad values
                    start <= end
                })
                .map(|(start, end, content)| {
                    Fragment {
                        start: start,
                        end: end,
                        content: content,
                    }
                }),
        )
    }
}

fn arbitrary_frag_content<G: Gen>(g: &mut G) -> Vec<u8> {
    let size = g.size();
    // Using a uniform distribution over size here can lead to extremely bloated
    // data structures. We also want to test zero-length data with more than a
    // (1/size) probability. So use a lognormal distribution.
    //
    // The choice of mean and stdev are pretty arbitrary, but they work well for
    // common sizes (~100).
    // TODO: make this more rigorous, e.g. by using params such that p95 = size.
    let lognormal = LogNormal::new(-3.0, 2.0);
    let content_len = ((size as f64) * lognormal.ind_sample(g)) as usize;

    let mut v = Vec::with_capacity(content_len);
    g.fill_bytes(&mut v);
    v
}

/// Apply a Delta to an input text, returning the result.
pub fn apply(text: &[u8], delta: Delta) -> Vec<u8> {
    let mut chunks = Vec::with_capacity(delta.frags.len() * 2);
    let mut off = 0;

    for frag in &delta.frags {
        assert!(off <= frag.start);
        if off < frag.start {
            chunks.push(&text[off..frag.start]);
        }
        if frag.content.len() > 0 {
            chunks.push(frag.content.as_ref())
        }
        off = frag.end;
    }
    if off < text.len() {
        chunks.push(&text[off..text.len()]);
    }

    let size = chunks.iter().map(|c| c.len()).sum::<usize>();
    let mut output = Vec::with_capacity(size);
    for c in chunks {
        output.extend_from_slice(c);
    }
    output
}

/// Apply a chain of Deltas to an input text, returning the result.
/// Should be faster than applying the Deltas one at a time since no
/// intermediate versions are produced.
pub fn apply_chain<I: IntoIterator<Item = Delta>>(text: &[u8], deltas: I) -> Vec<u8> {
    let combined = combine_chain(deltas);
    apply(text, combined)
}

/// Combine a chain of Deltas into an equivalent single Delta.
pub fn combine_chain<I: IntoIterator<Item = Delta>>(deltas: I) -> Delta {
    deltas.into_iter().fold(Delta::default(), combine)
}

/// Destructively combine two Deltas into a new Delta that is equivalent to
/// applying the original two Deltas in sequence.
pub fn combine(first: Delta, second: Delta) -> Delta {
    let mut combined = Vec::new();
    let mut first_frags = itertools::put_back(first.frags.into_iter());

    // Cumulative change in length caused by the fragments in `first` that have been
    // processed so far. We need to keep track of this because the offsets in `second`
    // are relative to the text after `first` is applied. We need to adjust
    // all of the offsets in `second` to compensate for this.
    let mut cum_len_change = 0;

    for mut frag in second.frags {
        // Take frags in `first` that occur before the current frag.
        let before = take_frags(
            Some(&mut combined),
            &mut first_frags,
            frag.start,
            cum_len_change,
        );

        // Skip frags in `first` that overlap the current frag.
        let after = take_frags(None, &mut first_frags, frag.end, before);

        // Adjust offsets in the new fragment to compensate for length changes caused by
        // the taken and skipped fragments respectively.
        frag.start = adjust(frag.start, before);
        frag.end = adjust(frag.end, after);

        combined.push(frag);
        cum_len_change = after;
    }

    // Add any remaining fragments from `first`.
    combined.extend(first_frags);

    Delta { frags: combined }
}

/// Move Fragments from src to dst until the given cutoff is reached. If the last Fragment
/// overlaps the cutoff, it will be split; the first half will be moved to dst while the
/// remainder will be put back into src. If dst is None, then the taken Fragments are dropped.
/// Returns the updated cumulative change of length that includes all of the taken fragments.
fn take_frags<I>(
    mut dst: Option<&mut Vec<Fragment>>,
    src: &mut PutBack<I>,
    cutoff: usize,
    mut cum_len_change: isize,
) -> isize
where
    I: Iterator<Item = Fragment>,
{
    while let Some(mut frag) = src.next() {
        // Adjust cutoff offset to account for the cumulative length change so far.
        let adjusted = adjust(cutoff, cum_len_change);

        // Does this fragment end after the cutoff?
        if frag.post_end() > adjusted {
            // Split the fragment if it starts before the cutoff.
            if let Some(rest) = frag.split(adjusted) {
                src.put_back(rest);
                cum_len_change += frag.length_change();
                dst.as_mut().map(|v| v.push(frag));
            } else {
                // Fragment started after the cutoff, so put it back.
                src.put_back(frag);
            }
            break;
        }

        // Push the fragment to the output and update the cumulative length change accordingly.
        cum_len_change += frag.length_change();
        dst.as_mut().map(|v| v.push(frag));
    }

    cum_len_change
}

/// Subtract the second (signed) value from the first (unsigned) value.
/// This function is here mostly to avoid cluttering the code with casts whenever
/// we need to adjust an offset.
fn adjust(offset: usize, adjustment: isize) -> usize {
    // XXX: Not explicitly checking for overflow/underflow since interger operations should
    // be checked in debug builds by default, as specified in RFC 560:
    // https://github.com/rust-lang/rfcs/pull/560
    // The alternative would be to use checked_add() and checked_sub() which would impose
    // a runtime cost in optimized builds, which is probably undesirable here.
    if adjustment < 0 {
        offset + (-adjustment) as usize
    } else {
        offset - adjustment as usize
    }
}

/// XXX: Comatibility functions for the old bdiff module for testing purposes. The delta
/// module will replace that one once all instances of Vec<bdiff::Delta> are replaced
/// with delta::Delta, and this compatibility module will be removed at that time.
pub mod compat {
    use super::*;
    use bdiff;

    pub fn convert<T>(deltas: T) -> Delta
    where
        T: IntoIterator<Item = bdiff::Delta>,
    {
        Delta {
            frags: deltas
                .into_iter()
                .map(|delta| {
                    Fragment {
                        start: delta.start,
                        end: delta.end,
                        content: delta.content.clone(),
                    }
                })
                .collect(),
        }
    }

    pub fn apply_deltas<T>(text: &[u8], deltas: T) -> Vec<u8>
    where
        T: IntoIterator<Item = Vec<bdiff::Delta>>,
    {
        apply_chain(text, deltas.into_iter().map(convert))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that fragments are verified properly.
    #[test]
    #[cfg_attr(rustfmt, rustfmt_skip)]
    fn test_delta_new() {
        let test_cases = vec![
            (vec![Fragment { start: 0, end: 0, content: vec![] }], true),
            (vec![Fragment { start: 0, end: 5, content: vec![] }], true),
            (vec![Fragment { start: 0, end: 5, content: vec![] },
                  Fragment { start: 5, end: 8, content: vec![] }], true),
            (vec![Fragment { start: 0, end: 5, content: vec![] },
                  Fragment { start: 6, end: 9, content: vec![] }], true),
            (vec![Fragment { start: 0, end: 5, content: vec![] },
                  Fragment { start: 6, end: 5, content: vec![] }], false),
            (vec![Fragment { start: 0, end: 5, content: vec![] },
                  Fragment { start: 4, end: 8, content: vec![] }], false),
        ];

        for (frags, success) in test_cases.into_iter() {
            let delta = Delta::new(frags);
            if success {
                assert!(delta.is_ok());
            } else {
                assert!(delta.is_err());
            }
        }
    }

    quickcheck! {
        fn delta_gen(delta: Delta) -> bool {
            Delta::verify(&delta.frags).is_ok()
        }

        fn delta_shrink(delta: Delta) -> bool {
            // This test is a bit redundant, but let's just verify.
            delta.shrink().take(100).all(|d| {
                Delta::verify(&d.frags).is_ok()
            })
        }

        fn fragment_gen(fragment: Fragment) -> bool {
            fragment.verify().is_ok()
        }

        fn fragment_shrink(fragment: Fragment) -> bool {
            fragment.shrink().take(100).all(|f| f.verify().is_ok())
        }
    }

    /// Test a fragment that decreases the size of the content.
    #[test]
    fn test_fragment_shrink() {
        let mut frag = Fragment {
            start: 10,
            end: 20,
            content: vec![1, 2, 3, 4, 5],
        };

        assert_eq!(frag.post_end(), 15);
        assert_eq!(frag.length_change(), -5);

        assert!(frag.contains_offset(12));
        assert!(!frag.contains_offset(17));

        assert_eq!(frag.split(17), None);
        let rest = frag.split(12).unwrap();

        assert_eq!(
            frag,
            Fragment {
                start: 10,
                end: 12,
                content: vec![1, 2],
            }
        );
        assert_eq!(
            rest,
            Fragment {
                start: 12,
                end: 20,
                content: vec![3, 4, 5],
            }
        );
    }

    /// Test a fragment that increases the size of the content.
    #[test]
    fn test_fragment_grow() {
        let mut frag = Fragment {
            start: 10,
            end: 15,
            content: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        };

        assert_eq!(frag.post_end(), 20);
        assert_eq!(frag.length_change(), 5);

        assert!(frag.contains_offset(17));

        // We're splitting within the content bounds of this Fragment, but after the end
        // offset. As a result, the split point should be the end offset, but the content
        // should still be split from the original split offset.
        let rest = frag.split(17).unwrap();

        assert_eq!(
            frag,
            Fragment {
                start: 10,
                end: 15,
                content: vec![1, 2, 3, 4, 5, 6, 7],
            }
        );
        assert_eq!(
            rest,
            Fragment {
                start: 15,
                end: 15,
                content: vec![8, 9, 10],
            }
        );
    }

    /// Test combining two Deltas with overlapping fragments.
    #[test]
    fn test_combine() {
        let delta1 = Delta {
            frags: vec![
                Fragment {
                    start: 3,
                    end: 6,
                    content: vec![1, 2, 3, 4, 5],
                },
                Fragment {
                    start: 8,
                    end: 16,
                    content: vec![6, 7, 8, 9],
                },
            ],
        };

        let delta2 = Delta {
            frags: vec![
                Fragment {
                    start: 7,
                    end: 12,
                    content: vec![10, 11, 12, 13],
                },
            ],
        };

        let expected = Delta {
            frags: vec![
                Fragment {
                    start: 3,
                    end: 6,
                    content: vec![1, 2, 3, 4],
                },
                Fragment {
                    start: 6,
                    end: 10,
                    content: vec![10, 11, 12, 13],
                },
                Fragment {
                    start: 10,
                    end: 16,
                    content: vec![8, 9],
                },
            ],
        };

        let combined = combine(delta1, delta2);
        assert_eq!(combined, expected);
    }

    #[test]
    fn test_apply_1() {
        let text = b"aaaa\nbbbb\ncccc\n";
        let delta = Delta {
            frags: vec![
                Fragment {
                    start: 5,
                    end: 10,
                    content: (&b"xxxx\n"[..]).into(),
                },
            ],
        };

        let res = apply(text, delta);
        assert_eq!(&res[..], b"aaaa\nxxxx\ncccc\n");
    }

    #[test]
    fn test_apply_2() {
        let text = b"bbbb\ncccc\n";
        let delta = Delta {
            frags: vec![
                Fragment {
                    start: 0,
                    end: 5,
                    content: (&b"aaaabbbb\n"[..]).into(),
                },
                Fragment {
                    start: 10,
                    end: 10,
                    content: (&b"dddd\n"[..]).into(),
                },
            ],
        };

        let res = apply(text, delta);
        assert_eq!(&res[..], b"aaaabbbb\ncccc\ndddd\n");
    }

    #[test]
    fn test_apply_3a() {
        let text = b"aaaa\nbbbb\ncccc\n";
        let delta = Delta {
            frags: vec![
                Fragment {
                    start: 0,
                    end: 15,
                    content: (&b"zzzz\nyyyy\nxxxx\n"[..]).into(),
                },
            ],
        };

        let res = apply(text, delta);
        assert_eq!(&res[..], b"zzzz\nyyyy\nxxxx\n");
    }

    #[test]
    fn test_apply_3b() {
        let text = b"aaaa\nbbbb\ncccc\n";
        let delta = Delta {
            frags: vec![
                Fragment {
                    start: 0,
                    end: 5,
                    content: (&b"zzzz\n"[..]).into(),
                },
                Fragment {
                    start: 5,
                    end: 10,
                    content: (&b"yyyy\n"[..]).into(),
                },
                Fragment {
                    start: 10,
                    end: 15,
                    content: (&b"xxxx\n"[..]).into(),
                },
            ],
        };

        let res = apply(text, delta);
        assert_eq!(&res[..], b"zzzz\nyyyy\nxxxx\n");
    }

    #[test]
    fn test_apply_4() {
        let text = b"aaaa\nbbbb";
        let delta = Delta {
            frags: vec![
                Fragment {
                    start: 5,
                    end: 9,
                    content: (&b"bbbbcccc"[..]).into(),
                },
            ],
        };

        let res = apply(text, delta);
        assert_eq!(&res[..], b"aaaa\nbbbbcccc");
    }

    #[test]
    fn test_apply_5() {
        let text = b"aaaa\nbbbb\ncccc\n";
        let delta = Delta {
            frags: vec![
                Fragment {
                    start: 5,
                    end: 10,
                    content: (&b""[..]).into(),
                },
            ],
        };

        let res = apply(text, delta);
        assert_eq!(&res[..], b"aaaa\ncccc\n");
    }
}
