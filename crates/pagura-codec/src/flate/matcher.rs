//! Bounded hash-chain LZ77 matching (RFC 1951 3.2.3/3.2.5 and section 4).
use super::Level;
use crate::encoding::filled;
use pagura_core::Result;

const WINDOW: usize = 32768;
const NONE: usize = usize::MAX;

#[derive(Clone, Copy, Debug)]
pub(super) struct Token {
    // A literal when distance=0, otherwise a match length.
    pub(super) value: u16,
    pub(super) distance: u16,
}

pub(super) struct Matcher {
    heads: Vec<usize>,
    previous: Vec<usize>,
    depth: usize,
    lazy: bool,
}

impl Matcher {
    pub(super) fn new(size: usize, level: Level) -> Result<Self> {
        Ok(Self {
            heads: filled(size.clamp(4, WINDOW).next_power_of_two(), NONE)?,
            previous: filled(size.min(WINDOW), NONE)?,
            depth: if level == Level::Fast { 32 } else { 128 },
            lazy: level == Level::Default,
        })
    }

    fn hash(&self, input: &[u8], pos: usize) -> Option<usize> {
        let bytes = input.get(pos..)?.get(..3)?;
        let value =
            (usize::from(bytes[0]) * 251 + usize::from(bytes[1])) * 251 + usize::from(bytes[2]);
        Some(value & (self.heads.len() - 1))
    }

    fn insert(&mut self, input: &[u8], pos: usize) -> usize {
        let Some(hash) = self.hash(input, pos) else {
            return NONE;
        };
        let old = self.heads[hash];
        self.previous[pos & (WINDOW - 1)] = old;
        self.heads[hash] = pos;
        old
    }

    fn find(&self, input: &[u8], pos: usize, mut candidate: usize, end: usize) -> (usize, usize) {
        let maximum = (end - pos).min(258);
        let mut best = (2, 0);
        if maximum < 3 {
            return best;
        }
        let Some(target) = input.get(pos..pos + maximum) else {
            return best;
        };
        for _ in 0..self.depth {
            if candidate >= pos || pos - candidate > WINDOW {
                break;
            }
            // Only strict improvements matter. The byte just after the best
            // prefix rejects ties without rescanning that prefix. A maximum
            // length match exits below, so best.0 is smaller than maximum.
            if let Some(source) = input.get(candidate..candidate + maximum)
                && source[best.0 - 1] == target[best.0 - 1]
                && source[best.0] == target[best.0]
            {
                let length = common_prefix(source, target);
                if length > best.0 {
                    best = (length, pos - candidate);
                    if length == maximum {
                        break;
                    }
                }
            }
            let next = self.previous[candidate & (WINDOW - 1)];
            // Ring reuse can destroy the oldest link; never follow it forward.
            if next >= candidate {
                break;
            }
            candidate = next;
        }
        best
    }

    pub(super) fn tokenize(
        &mut self,
        input: &[u8],
        start: usize,
        end: usize,
        tokens: &mut Vec<Token>,
    ) {
        tokens.clear();
        let mut pos = start;
        while pos < end {
            let old = self.insert(input, pos);
            let best = self.find(input, pos, old, end);
            if best.0 >= 3 {
                if self.lazy && best.0 < 258 && pos + 1 < end {
                    // Look ahead without inserting: the current position is
                    // already present, and the next iteration will insert once.
                    let next = self
                        .hash(input, pos + 1)
                        .map_or(NONE, |hash| self.heads[hash]);
                    if self.find(input, pos + 1, next, end).0 > best.0 {
                        tokens.push(Token {
                            value: u16::from(input[pos]),
                            distance: 0,
                        });
                        pos += 1;
                        continue;
                    }
                }
                tokens.push(match_token(best.0, best.1));
                for skipped in pos + 1..pos + best.0 {
                    self.insert(input, skipped);
                }
                pos += best.0;
            } else {
                tokens.push(Token {
                    value: u16::from(input[pos]),
                    distance: 0,
                });
                pos += 1;
            }
        }
    }
}

fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    // Fixed-size comparisons let the compiler compare full chunks together;
    // inspect individual bytes only inside the first unequal chunk and tail.
    let (left_chunks, left_tail) = left.as_chunks::<16>();
    let (right_chunks, right_tail) = right.as_chunks::<16>();
    for (index, (a, b)) in left_chunks.iter().zip(right_chunks).enumerate() {
        if a != b {
            return index * 16 + a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(16);
        }
    }
    left_chunks.len() * 16
        + left_tail
            .iter()
            .zip(right_tail)
            .position(|(a, b)| a != b)
            .unwrap_or(left_tail.len())
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "matches are at most 258 bytes and distances at most 32768"
)]
fn match_token(length: usize, distance: usize) -> Token {
    Token {
        value: length as u16,
        distance: distance as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::common_prefix;

    #[test]
    fn chunk_comparison_stops_at_every_byte_including_tails() {
        for length in 0..=258 {
            let left = vec![42; length];
            assert_eq!(common_prefix(&left, &left), length);
            for mismatch in 0..length {
                let mut right = left.clone();
                right[mismatch] ^= 1;
                assert_eq!(common_prefix(&left, &right), mismatch);
            }
        }
    }
}
