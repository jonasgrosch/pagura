//! Reservations for insert-only standard-library hash tables. These are
//! conservative retained-byte estimates, not allocator telemetry.
//!
//! Rust 1.99.1 uses hashbrown 0.17.1: a power-of-two bucket array with at
//! least one eighth spare, control bytes and a control-group tail. Small
//! entries can require a larger minimum table. The estimate allows 16-byte
//! control groups, 32 tail bytes and type alignment padding on every target.
//! A toolchain upgrade must revalidate the load envelope, small-table
//! minima, control-group width, layout padding and growth behavior against
//! the native and both WASM target capacity tests below. No table layout or
//! allocator implementation is exposed to callers.

use super::Meter;
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash};

impl Meter {
    /// Makes room for `additional` map entries, charging table capacity.
    ///
    /// The map must start empty and only grow through this method: its
    /// existing estimated capacity is already charged, and entries must not
    /// be removed, since tombstones can reduce the reported capacity. Keys'
    /// and values' separate allocations are charged by their owner. Replacement storage
    /// is charged while the old table is still live, then the old estimate
    /// and unused allowance are returned after successful reservation.
    ///
    /// # Errors
    ///
    /// Returns this meter's `Limit` error before the reservation exceeds its
    /// allowance, or when checked sizing or fallible reservation fails.
    pub fn reserve_map_capacity<K: Eq + Hash, V, S: BuildHasher>(
        &mut self,
        values: &mut HashMap<K, V, S>,
        additional: usize,
    ) -> Result<()> {
        let Some((additional, reserved, old)) =
            self.table_reservation::<(K, V)>(values.len(), values.capacity(), additional)?
        else {
            return Ok(());
        };
        values
            .try_reserve(additional)
            .map_err(|_| Error::limit(self.message))?;
        let retained = self.map_bytes(values)?;
        self.finish_table_reservation(reserved, old, retained)
    }

    /// [`Self::reserve_map_capacity`] for a set, with the same insert-only,
    /// already-charged-capacity requirement and byte estimate.
    ///
    /// # Errors
    ///
    /// As [`Self::reserve_map_capacity`].
    pub fn reserve_set_capacity<T: Eq + Hash, S: BuildHasher>(
        &mut self,
        values: &mut HashSet<T, S>,
        additional: usize,
    ) -> Result<()> {
        let Some((additional, reserved, old)) =
            self.table_reservation::<(T, ())>(values.len(), values.capacity(), additional)?
        else {
            return Ok(());
        };
        values
            .try_reserve(additional)
            .map_err(|_| Error::limit(self.message))?;
        let retained = self.set_bytes(values)?;
        self.finish_table_reservation(reserved, old, retained)
    }

    /// The conservative table bytes charged by [`Self::reserve_map_capacity`],
    /// excluding values' separate allocations. Drop the map before refunding
    /// this amount. The insert-only requirement of that method applies.
    ///
    /// # Errors
    ///
    /// Returns this meter's `Limit` error if checked sizing overflows.
    pub fn map_bytes<K, V, S>(&self, values: &HashMap<K, V, S>) -> Result<usize> {
        table_bytes::<(K, V)>(values.capacity()).ok_or(Error::limit(self.message))
    }

    /// [`Self::map_bytes`] for an insert-only set.
    ///
    /// # Errors
    ///
    /// As [`Self::map_bytes`].
    pub fn set_bytes<T, S>(&self, values: &HashSet<T, S>) -> Result<usize> {
        table_bytes::<(T, ())>(values.capacity()).ok_or(Error::limit(self.message))
    }

    fn table_reservation<T>(
        &mut self,
        len: usize,
        capacity: usize,
        additional: usize,
    ) -> Result<Option<(usize, usize, usize)>> {
        let need = len
            .checked_add(additional)
            .ok_or(Error::limit(self.message))?;
        if need <= capacity {
            return Ok(None);
        }
        // Request the policy minimum as well as charging it, so small
        // entries do not get different growth points on 8- and 16-byte
        // control-group targets.
        let minimum = match size_of::<T>() {
            0..=1 => 14,
            2..=3 => 7,
            _ => 3,
        };
        let exact = need.max(minimum);
        let preferred = exact.max(capacity.saturating_mul(2));
        let target = if table_bytes::<T>(preferred).is_some_and(|bytes| bytes <= self.remaining()) {
            preferred
        } else {
            exact
        };
        let reserved = table_bytes::<T>(target).ok_or(Error::limit(self.message))?;
        let old = table_bytes::<T>(capacity).ok_or(Error::limit(self.message))?;
        self.charge(reserved)?;
        Ok(Some((target - len, reserved, old)))
    }

    fn finish_table_reservation(
        &mut self,
        reserved: usize,
        old: usize,
        retained: usize,
    ) -> Result<()> {
        // A supported std table's observed growth stays within the
        // pre-reserved envelope. Refuse if a changed implementation exceeds
        // the policy; never silently accept an uncharged retained table.
        let unused = reserved
            .checked_sub(retained)
            .ok_or(Error::limit(self.message))?;
        let released = old.checked_add(unused).ok_or(Error::limit(self.message))?;
        self.refund(released);
        Ok(())
    }
}

fn table_bytes<T>(capacity: usize) -> Option<usize> {
    if capacity == 0 {
        return Some(0);
    }
    let minimum = match size_of::<T>() {
        0..=1 => 16,
        2..=3 => 8,
        _ => 4,
    };
    let load = capacity.checked_mul(8)?.checked_add(6)? / 7;
    let buckets = load.checked_next_power_of_two()?.max(minimum);
    buckets
        .checked_mul(size_of::<T>().checked_add(1)?)?
        .checked_add(32)?
        .checked_add(align_of::<T>().max(16))
}

#[cfg(test)]
mod tests {
    use super::table_bytes;
    use crate::hash::{FastMap, FastSet};
    use crate::{ErrorKind, Meter};

    const MESSAGE: &str = "test table bytes";

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    #[repr(align(64))]
    struct Aligned(u32);

    #[test]
    fn capacity_estimates_cover_native_table_growth_and_alignment() {
        fn check<K: Eq + std::hash::Hash>(key: impl Fn(u32) -> K) {
            let mut map = FastMap::default();
            let mut meter = Meter::new(1 << 24, MESSAGE);
            assert_eq!(meter.map_bytes(&map).unwrap(), 0);
            for i in 0..4096 {
                meter.reserve_map_capacity(&mut map, 1).unwrap();
                map.insert(key(i), i);
                assert_eq!(meter.used(), meter.map_bytes(&map).unwrap());
                assert!(meter.used() >= map.capacity() * size_of::<(K, u32)>());
            }
            let released = meter.map_bytes(&map).unwrap();
            drop(map);
            meter.refund(released);
            assert_eq!(meter.used(), 0);
        }
        check(|i| i);
        check(Aligned);
        check(|i| [i; 33]);
    }

    #[test]
    fn tiny_and_zero_sized_sets_fit_the_small_table_envelope() {
        let mut bytes = Meter::new(1 << 20, MESSAGE);
        let mut tiny = FastSet::default();
        for i in 0..=u8::MAX {
            bytes.reserve_set_capacity(&mut tiny, 1).unwrap();
            tiny.insert(i);
            assert_eq!(bytes.used(), bytes.set_bytes(&tiny).unwrap());
        }
        let mut empty_value = FastSet::default();
        let before = bytes.used();
        bytes.reserve_set_capacity(&mut empty_value, 1).unwrap();
        empty_value.insert(());
        assert_eq!(
            bytes.used() - before,
            bytes.set_bytes(&empty_value).unwrap()
        );
    }

    #[test]
    fn growth_refusal_preserves_contents_and_capacity_before_reservation() {
        let initial = table_bytes::<(u32, u32)>(1).unwrap();
        let replacement = table_bytes::<(u32, u32)>(4).unwrap();
        let mut empty = FastMap::<u32, u32>::default();
        let mut tiny = Meter::new(initial - 1, MESSAGE);
        assert_eq!(
            tiny.reserve_map_capacity(&mut empty, 1).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!((empty.capacity(), tiny.used()), (0, 0));
        let mut bytes = Meter::new(initial + replacement - 1, MESSAGE);
        let mut map = FastMap::default();
        bytes.reserve_map_capacity(&mut map, 3).unwrap();
        map.extend([(0, 0), (1, 1), (2, 2)]);
        let capacity = map.capacity();
        assert_eq!(bytes.used(), initial);
        let error = bytes.reserve_map_capacity(&mut map, 1).unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Limit, MESSAGE));
        assert_eq!(
            (map.len(), map.capacity(), bytes.used()),
            (3, capacity, initial)
        );

        let mut bytes = Meter::new(initial + replacement, MESSAGE);
        bytes.charge(initial).unwrap();
        bytes.reserve_map_capacity(&mut map, 1).unwrap();
        assert_eq!(bytes.used(), bytes.map_bytes(&map).unwrap());
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn empty_overflow_and_unused_capacity_do_not_allocate_or_double_charge() {
        let mut map: FastMap<u32, u32> = FastMap::default();
        let mut bytes = Meter::new(usize::MAX, MESSAGE);
        bytes.reserve_map_capacity(&mut map, 0).unwrap();
        assert_eq!((map.capacity(), bytes.used()), (0, 0));
        assert_eq!(
            bytes
                .reserve_map_capacity(&mut map, usize::MAX)
                .unwrap_err()
                .kind,
            ErrorKind::Limit
        );
        assert_eq!((map.capacity(), bytes.used()), (0, 0));
        bytes.reserve_map_capacity(&mut map, 8).unwrap();
        let before = bytes.used();
        bytes.reserve_map_capacity(&mut map, 8).unwrap();
        assert_eq!(bytes.used(), before);
        assert!(table_bytes::<Aligned>(usize::MAX / 128).is_none());
    }
}
