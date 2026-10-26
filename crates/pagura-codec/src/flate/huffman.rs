//! Length-limited canonical codes (RFC 1951 3.2.2/3.2.7).
//!
//! Binary package-merge selects the cheapest 2*n-2 items at the last level.
//! Each selected occurrence of a leaf contributes one bit to its code length.
//! Stable weight/creation-order ties make the construction deterministic.
use crate::encoding::limit;
use pagura_core::{Meter, Result};

#[derive(Clone, Copy)]
struct Node {
    weight: u64,
    symbol: usize,
    children: Option<[usize; 2]>,
}

/// One preallocated package-merge workspace, reused for all three alphabets.
pub(super) struct Workspace {
    nodes: Vec<Node>,
    leaves: Vec<usize>,
    current: Vec<usize>,
    next: Vec<usize>,
    packages: Vec<usize>,
}

impl Workspace {
    pub(super) fn new(bytes: &mut Meter) -> Result<Self> {
        let mut workspace = Self {
            nodes: Vec::new(),
            leaves: Vec::new(),
            current: Vec::new(),
            next: Vec::new(),
            packages: Vec::new(),
        };
        bytes.reserve_capacity(&mut workspace.nodes, 286 * 15)?;
        bytes.reserve_capacity(&mut workspace.leaves, 286)?;
        bytes.reserve_capacity(&mut workspace.current, 2 * 286 + 15)?;
        bytes.reserve_capacity(&mut workspace.next, 2 * 286 + 15)?;
        bytes.reserve_capacity(&mut workspace.packages, 286)?;
        Ok(workspace)
    }
}

pub(super) struct Book<const N: usize> {
    pub(super) lengths: [u8; N],
    pub(super) codes: [u16; N],
}

impl<const N: usize> Book<N> {
    pub(super) fn from_lengths(lengths: [u8; N]) -> Self {
        let mut counts = [0_u16; 16];
        for &len in &lengths {
            if len != 0 {
                counts[usize::from(len)] += 1;
            }
        }
        let mut next = [0_u16; 16];
        for bits in 2..16 {
            next[bits] = (next[bits - 1] + counts[bits - 1]) << 1;
        }
        let mut codes = [0; N];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let code = &mut next[usize::from(len)];
                codes[symbol] = code.reverse_bits() >> (16 - len);
                *code += 1;
            }
        }
        Self { lengths, codes }
    }

    pub(super) fn build(
        frequencies: &[u32; N],
        max_bits: u8,
        workspace: &mut Workspace,
    ) -> Result<Self> {
        if !(2..=286).contains(&N) || !(1..=15).contains(&max_bits) {
            return Err(limit(0));
        }
        let count = frequencies.iter().filter(|&&f| f != 0).count().max(2);
        if count > (1 << max_bits) {
            return Err(limit(0));
        }
        let Workspace {
            nodes,
            leaves,
            current,
            next,
            packages,
        } = workspace;
        nodes.clear();
        for (symbol, &weight) in frequencies.iter().enumerate() {
            if weight != 0 {
                nodes.push(Node {
                    weight: u64::from(weight),
                    symbol,
                    children: None,
                });
            }
        }
        // A dummy symbol makes even empty/single-symbol alphabets complete.
        // Its frequency cannot affect the cost of any real input symbol.
        for (symbol, &frequency) in frequencies.iter().enumerate() {
            if nodes.len() >= 2 {
                break;
            }
            if frequency == 0 {
                nodes.push(Node {
                    weight: 0,
                    symbol,
                    children: None,
                });
            }
        }
        leaves.clear();
        leaves.extend(0..count);
        leaves.sort_unstable_by_key(|&id| (nodes[id].weight, id));
        current.clear();
        current.extend_from_slice(leaves);
        for _ in 1..max_bits {
            packages.clear();
            for pair in current.as_chunks::<2>().0 {
                let id = nodes.len();
                nodes.push(Node {
                    weight: nodes[pair[0]].weight + nodes[pair[1]].weight,
                    symbol: 0,
                    children: Some([pair[0], pair[1]]),
                });
                packages.push(id);
            }
            next.clear();
            let (mut leaf, mut package) = (0, 0);
            while leaf < leaves.len() || package < packages.len() {
                let take_leaf = package == packages.len()
                    || (leaf < leaves.len()
                        && (nodes[leaves[leaf]].weight, leaves[leaf])
                            <= (nodes[packages[package]].weight, packages[package]));
                if take_leaf {
                    next.push(leaves[leaf]);
                    leaf += 1;
                } else {
                    next.push(packages[package]);
                    package += 1;
                }
            }
            std::mem::swap(current, next);
        }
        let selected = current.get(..2 * count - 2).ok_or_else(|| limit(0))?;
        let mut lengths = [0; N];
        // The stack has at most max_bits pending siblings plus selected roots.
        next.clear();
        next.extend_from_slice(selected);
        while let Some(id) = next.pop() {
            let node = nodes[id];
            if let Some(children) = node.children {
                next.extend_from_slice(&children);
            } else {
                lengths[node.symbol] += 1;
            }
        }
        Ok(Self::from_lengths(lengths))
    }
}

#[cfg(test)]
mod tests {
    use super::{Book, Workspace};
    use pagura_core::Meter;

    fn build<const N: usize>(frequencies: &[u32; N], max: u8) -> pagura_core::Result<Book<N>> {
        let mut bytes = Meter::new(usize::MAX, "test workspace");
        Book::build(frequencies, max, &mut Workspace::new(&mut bytes)?)
    }

    #[test]
    fn package_merge_matches_exhaustive_small_oracle() {
        // Independently enumerate every complete tree length assignment.
        for frequencies in [[1, 1, 1, 1], [1, 2, 4, 8], [1, 1, 1, 99], [10, 2, 3, 4]] {
            for max in 2..=4 {
                let book = build(&frequencies, max).unwrap();
                let actual: u32 = frequencies
                    .iter()
                    .zip(book.lengths)
                    .map(|(&f, l)| f * u32::from(l))
                    .sum();
                let mut best = u32::MAX;
                for a in 1..=max {
                    for b in 1..=max {
                        for c in 1..=max {
                            for d in 1..=max {
                                let lengths = [a, b, c, d];
                                if lengths.iter().map(|&l| 1_u32 << (max - l)).sum::<u32>()
                                    == 1 << max
                                {
                                    let cost = frequencies
                                        .iter()
                                        .zip(lengths)
                                        .map(|(&f, l)| f * u32::from(l))
                                        .sum();
                                    best = best.min(cost);
                                }
                            }
                        }
                    }
                }
                assert_eq!(actual, best);
            }
        }
    }

    #[test]
    fn skewed_alphabets_are_complete_and_length_limited() {
        let mut frequencies = [0_u32; 286];
        frequencies[0] = 1;
        frequencies[1] = 1;
        for i in 2..30 {
            frequencies[i] = frequencies[i - 1] + frequencies[i - 2];
        }
        for max in [7, 15] {
            let book = build(&frequencies, max).unwrap();
            assert!(book.lengths.iter().all(|&l| l <= max));
            assert!(book.lengths[..30].iter().all(|&l| l > 0));
            assert_eq!(
                book.lengths
                    .iter()
                    .filter(|&&l| l > 0)
                    .map(|&l| 1_u32 << (max - l))
                    .sum::<u32>(),
                1 << max
            );
            assert!(book.lengths[30..].iter().all(|&l| l == 0));
        }
        assert_eq!(
            build(&[0_u32; 19], 7)
                .unwrap()
                .lengths
                .iter()
                .filter(|&&l| l > 0)
                .count(),
            2
        );
        let mut single = [0; 286];
        single[256] = 1;
        let book = build(&single, 15).unwrap();
        assert_eq!(book.lengths[256], 1);
        assert_eq!(book.lengths.iter().filter(|&&l| l > 0).count(), 2);
    }
}
