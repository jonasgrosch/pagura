//! Reuses a complete ordered frontier only after every adjacent relation
//! is proved over its whole shared height interval. Strict relations use
//! full-source error radii; identical functions retain every signed member.
//! No current-key cache is part of that certificate. A failed attempt leaves
//! the resolver untouched and takes its original placement and joining path.
//!
//! Only a plausible one-ending/one-entering boundary can establish the
//! certificate. Its two new neighbors are proved first and omitted from the
//! unchanged scan, which stops on the first failure: at most K-1 relations
//! for K charged old-active visits. The first visit also bounds the fixed
//! event/publication gates. Every center cache has eight fixed entries;
//! rules are bounded to 256 on this path. Failed attempts include the full
//! original fallback in the same fixed-body bound, with no additional
//! input-sized traversal or heap owner. Old logical charges stay unchanged.

use super::{
    Line, Meter, Open, Piece, Place, Resolver, Result, Span, key_radius, push_side, room, sort_work,
};

pub(super) const SINGLETON: u32 = 1 << 31;
const POSITION: u32 = SINGLETON - 1;

/// One sweep's event cursor and proof state. The ending events reuse columns
/// after its last crossing consumer; there is no retained owner or epoch.
pub(super) struct Sweep {
    enabled: bool,
    certified: bool,
    previous: Option<usize>,
    ended: usize,
    replaced: bool,
}

impl Sweep {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            certified: false,
            previous: None,
            ended: 0,
            replaced: false,
        }
    }

    pub(super) fn replaced(&self) -> bool {
        self.replaced
    }

    pub(super) fn fallback(&mut self, expired: usize, segment: usize) {
        self.ended += expired;
        self.previous = Some(segment);
        self.certified = false;
    }

    /// The original old-active visit fee has already been paid. All guards
    /// and proofs are read-only. Publication begins only after the original
    /// joining/slab fees and the one possible original side append succeed.
    pub(super) fn attempt(
        &mut self,
        resolver: &mut Resolver,
        pieces: &[Piece],
        (a, b): (f64, f64),
        joined: usize,
        segment: usize,
        (out, work, bytes): (&mut Vec<Line>, &mut Meter, &mut Meter),
    ) -> Result<bool> {
        self.replaced = false;
        if !self.enabled
            || self.previous != Some(segment)
            || resolver.order.is_empty()
            // General join swaps these owners on every slab. Equal
            // capacities make skipping that swap preserve future reserve
            // and refusal history, including reused/failed scratch.
            || resolver.open.capacity() != resolver.next.capacity()
        {
            return Ok(false);
        }
        let ending = resolver
            .columns
            .get(self.ended)
            .filter(|event| event.0 <= a);
        let entering = resolver.rows.get(joined).filter(|event| event.0 <= a);
        if ending.is_none() && entering.is_none() {
            if !self.certified {
                return Ok(false);
            }
            work.charge(
                resolver
                    .order
                    .len()
                    .saturating_add(resolver.open.len())
                    .saturating_add(1),
            )?;
            #[cfg(test)]
            {
                resolver.frontier_successes += 1;
            }
            return Ok(true);
        }
        let Some(replacement) = replacement(
            resolver,
            pieces,
            (a, b),
            (self.ended, joined),
            self.certified,
        ) else {
            return Ok(false);
        };
        let Replacement {
            old_id,
            new_id,
            position,
            side,
        } = replacement;
        let old = &pieces[old_id as usize];
        // An exact eager joiner is retained, including all original key
        // arithmetic before its original joining charge. Strict neighbors
        // prove no survivor swap and the same insertion position.
        let entry = Place::new(new_id, pieces, a.midpoint(b));
        work.charge(sort_work(1).saturating_add(resolver.order.len() - 1))?;
        work.charge(
            resolver
                .order
                .len()
                .saturating_add(resolver.open.len())
                .saturating_add(1),
        )?;
        if let Some((_, sign, from)) = side {
            push_side((out, bytes), old.line, sign, (from, a))?;
        }
        resolver.order[position] = entry;
        resolver.joining.clear();
        resolver.joining.push(entry);
        let slot = &mut resolver.slots[new_id as usize];
        slot.position = u32::try_from(position).unwrap_or(u32::MAX) | SINGLETON;
        slot.side_position = u16::MAX;
        if let Some((index, sign, _)) = side {
            resolver.sides[index] = (new_id, sign);
            resolver.open[index] = Open {
                piece: new_id,
                side: sign,
                from: a,
            };
            slot.side_position = u16::try_from(index).unwrap_or(u16::MAX);
        }
        self.ended += 1;
        self.certified = true;
        self.replaced = true;
        #[cfg(test)]
        {
            resolver.frontier_successes += 1;
            resolver.frontier_replacements += 1;
        }
        Ok(true)
    }
}

/// Matching stored endpoints are a conservative admission gate. No endpoint
/// is synthesized, and sides never extend an old line through the handoff.
fn meeting(old: &Piece, new: &Piece) -> bool {
    let old = if old.line.a.y < old.line.b.y {
        old.line.b
    } else {
        old.line.a
    };
    let new = if new.line.a.y < new.line.b.y {
        new.line.a
    } else {
        new.line.b
    };
    old.x.to_bits() == new.x.to_bits() && old.y.to_bits() == new.y.to_bits()
}

#[derive(Clone, Copy)]
struct View {
    id: u32,
    code: u8,
}

impl View {
    fn old(place: &Place) -> Self {
        Self {
            id: place.piece,
            code: place.radius_code,
        }
    }
    fn new(id: u32, pieces: &[Piece]) -> Self {
        Self {
            id,
            code: key_radius(&pieces[id as usize]),
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    id: u32,
    y: u64,
    value: f64,
}

struct Centers {
    entries: [Entry; 8],
    len: usize,
}

impl Centers {
    fn new() -> Self {
        Self {
            entries: [Entry {
                id: u32::MAX,
                y: 0,
                value: 0.0,
            }; 8],
            len: 0,
        }
    }

    fn value(&mut self, view: View, y: f64, pieces: &[Piece]) -> Option<f64> {
        for entry in &self.entries[..self.len] {
            if entry.id == view.id && entry.y == y.to_bits() {
                return Some(entry.value);
            }
        }
        let line = &pieces[view.id as usize].line;
        let height = line.b.y - line.a.y;
        let offset = y - line.a.y;
        let fraction = offset / height;
        let width = line.b.x - line.a.x;
        let product = width * fraction;
        let value = line.a.x + product;
        if !value.is_finite() || self.len == self.entries.len() {
            return None;
        }
        self.entries[self.len] = Entry {
            id: view.id,
            y: y.to_bits(),
            value,
        };
        self.len += 1;
        Some(value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Relation {
    Strict,
    Identical,
}

fn normalized(piece: &Piece) -> [u64; 4] {
    let line = &piece.line;
    let (a, b) = if line.a.y < line.b.y {
        (&line.a, &line.b)
    } else {
        (&line.b, &line.a)
    };
    [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()]
}

/// The six-operation centers are within 16*u*M of the real affine line;
/// the literal compensated keys are within 384*u*M. Full-source R>500*u*M
/// thus transports endpoint center gaps through the affine interval with
/// margin for every interior literal key. Rounded centers need not be
/// monotone. Radius sums are exact powers-of-two sums in this domain.
fn full_relation(
    p: View,
    q: View,
    pieces: &[Piece],
    allow_identity: bool,
    centers: &mut Centers,
) -> Option<Relation> {
    let (left, right) = (&pieces[p.id as usize], &pieces[q.id as usize]);
    let (from, to) = (left.lo.max(right.lo), left.hi.min(right.hi));
    if !(from.is_finite() && to.is_finite() && from < to) {
        return None;
    }
    let (a, b) = (&left.line, &right.line);
    if allow_identity
        && a.a.x.is_finite()
        && a.a.x.to_bits() == a.b.x.to_bits()
        && a.a.x.to_bits() == b.a.x.to_bits()
        && b.a.x.to_bits() == b.b.x.to_bits()
    {
        return Some(Relation::Identical);
    }
    if p.code == 0 || q.code == 0 {
        return None;
    }
    if allow_identity && normalized(left) == normalized(right) {
        return Some(Relation::Identical);
    }
    let radius = f64::from_bits((u64::from(p.code) + 979) << 52)
        + f64::from_bits((u64::from(q.code) + 979) << 52);
    let p0 = centers.value(p, from, pieces)?;
    let q0 = centers.value(q, from, pieces)?;
    let p1 = centers.value(p, to, pieces)?;
    let q1 = centers.value(q, to, pieces)?;
    (p0 - q0 < -radius && p1 - q1 < -radius).then_some(Relation::Strict)
}

fn replacement_neighbors(order: &[Place], position: usize, new: View, pieces: &[Piece]) -> bool {
    let mut centers = Centers::new();
    if position > 0
        && full_relation(
            View::old(&order[position - 1]),
            new,
            pieces,
            false,
            &mut centers,
        ) != Some(Relation::Strict)
    {
        return false;
    }
    if position + 1 < order.len()
        && full_relation(
            new,
            View::old(&order[position + 1]),
            pieces,
            false,
            &mut centers,
        ) != Some(Relation::Strict)
    {
        return false;
    }
    centers.len <= 7
}

fn establish_unchanged(order: &[Place], position: usize, pieces: &[Piece]) -> bool {
    for right in 1..order.len() {
        if right == position || right - 1 == position {
            continue;
        }
        let mut centers = Centers::new();
        if full_relation(
            View::old(&order[right - 1]),
            View::old(&order[right]),
            pieces,
            true,
            &mut centers,
        )
        .is_none()
        {
            return false;
        }
    }
    true
}

/// Backward merge into the former full height reservation. Every original
/// f64 bit occurs once before numeric dedup, including both signs of zero.
pub(super) fn merge_heights(heights: &mut Vec<f64>, starts: &[Span], ends: &[Span]) {
    let (mut i, mut j, mut n) = (heights.len(), starts.len(), ends.len());
    debug_assert!(heights.capacity() >= i + j + n);
    heights.resize(i + j + n, 0.0);
    for k in (0..heights.len()).rev() {
        if j == 0 && n == 0 {
            break;
        }
        let extra = i.checked_sub(1).map(|index| heights[index]);
        let start = j.checked_sub(1).map(|index| starts[index].0);
        let end = n.checked_sub(1).map(|index| ends[index].0);
        if extra.is_some_and(|x| {
            start.is_none_or(|y| !x.total_cmp(&y).is_lt())
                && end.is_none_or(|y| !x.total_cmp(&y).is_lt())
        }) {
            i -= 1;
            heights[k] = heights[i];
        } else if start.is_some_and(|x| end.is_none_or(|y| !x.total_cmp(&y).is_lt())) {
            j -= 1;
            heights[k] = starts[j].0;
        } else {
            n -= 1;
            heights[k] = ends[n].0;
        }
    }
}

/// Preserves the former effective height reservation at every insertion.
/// The ending-table write replaces the old explicit-hi visit; two smaller
/// sorts fit the former extra+hi sort packet before one three-way merge.
pub(super) fn heights(
    resolver: &mut Resolver,
    pieces: &[Piece],
    total: u32,
    top: f64,
    (work, bytes): (&mut Meter, &mut Meter),
) -> Result<bool> {
    let count = pieces.len();
    let breaks = resolver.carry.len().saturating_sub(1);
    room(&mut resolver.heights, 2 * count + 2 + breaks, bytes)?;
    resolver.heights.clear();
    resolver.heights.extend([top, top + 1.0]);
    resolver.heights.extend_from_slice(&resolver.carry[1..]);
    // Both endpoint lists already have a scratch owner. Keep them
    // implicit through crossing insertion, then merge the three sorted
    // streams into exactly the original height slots. The test oracle
    // retains the former explicit-end layout and its original sorts.
    let frontier = resolver.frontier_enabled();
    let implicit = if frontier { 2 * count } else { count };
    #[cfg(test)]
    let implicit = if resolver.original_height_sort {
        0
    } else {
        implicit
    };
    if !frontier {
        for p in pieces {
            #[cfg(test)]
            if resolver.original_height_sort {
                resolver.heights.push(p.lo);
            }
            resolver.heights.push(p.hi);
        }
    }
    resolver.crossings(pieces, total, implicit, (work, bytes))?;
    work.charge(sort_work(resolver.heights.len() + implicit))?;
    resolver.heights.sort_unstable_by(f64::total_cmp);
    let mut finite_grid = top.is_finite() && top + 1.0 > top;
    if frontier {
        for (i, p) in (0..total).zip(pieces) {
            resolver.columns[i as usize] = (p.hi, p.lo, i);
            finite_grid &= p.lo.is_finite() && p.hi.is_finite() && p.lo < p.hi;
        }
        resolver
            .columns
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        merge_heights(&mut resolver.heights, &resolver.rows, &resolver.columns);
    } else if implicit != 0 {
        super::merge_heights(&mut resolver.heights, &resolver.rows);
    }
    resolver.heights.dedup();
    Ok(frontier && finite_grid)
}

struct Replacement {
    old_id: u32,
    new_id: u32,
    position: usize,
    side: Option<(usize, i32, f64)>,
}

/// Pure prospective gate. The new neighbors establish singleton/rank; they
/// are skipped in the unchanged scan, so total relation calls are <=K-1.
fn replacement(
    resolver: &Resolver,
    pieces: &[Piece],
    (a, b): (f64, f64),
    (ended, joined): (usize, usize),
    certified: bool,
) -> Option<Replacement> {
    let &(_, _, old_id) = resolver.columns.get(ended)?;
    let &(_, hi, new_id) = resolver.rows.get(joined)?;
    if resolver
        .columns
        .get(ended + 1)
        .is_some_and(|event| event.0 <= a)
        || resolver
            .rows
            .get(joined + 1)
            .is_some_and(|event| event.0 <= a)
        || hi < b
    {
        return None;
    }
    let (old, new) = (&pieces[old_id as usize], &pieces[new_id as usize]);
    if old.hi != a
        || new.lo != a
        || old.group != new.group
        || old.direction != new.direction
        || !meeting(old, new)
    {
        return None;
    }
    let slot = &resolver.slots[old_id as usize];
    let position = (slot.position & POSITION) as usize;
    if slot.position & SINGLETON == 0
        || resolver
            .order
            .get(position)
            .is_none_or(|entry| entry.piece != old_id)
    {
        return None;
    }
    let side = if slot.side_position == u16::MAX {
        None
    } else {
        let index = usize::from(slot.side_position);
        let &(id, sign) = resolver.sides.get(index)?;
        let open = resolver.open.get(index)?;
        if id != old_id || open.piece != old_id || open.side != sign {
            return None;
        }
        Some((index, sign, open.from))
    };
    let new_view = View::new(new_id, pieces);
    if new_view.code == 0
        || !replacement_neighbors(&resolver.order, position, new_view, pieces)
        || (!certified && !establish_unchanged(&resolver.order, position, pieces))
    {
        return None;
    }
    Some(Replacement {
        old_id,
        new_id,
        position,
        side,
    })
}
