//! Storage that preparing a shape needs only while it prepares, kept for
//! the shapes after it.

use super::certify::{PolygonBuffers, SplineBuffers};
use pagura_core::{Meter, Result};
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

/// Buffers that the winding certificates of a render's shapes take turns
/// with, so that they share their allocations.
///
/// A use is charged as if its buffers were new: kept capacity changes no
/// charge, so a shape is refused exactly where it would be without them.
/// What is kept between uses is charged to no shape; it stays below
/// 100 KiB, because certificates of more than 256 vertices or pieces make
/// their own buffers.
#[derive(Debug, Default)]
pub struct ShapeScratch {
    pub(super) polygon: RefCell<PolygonBuffers>,
    pub(super) spline: RefCell<SplineBuffers>,
}

/// The most vertices or pieces of a certificate that takes turns with the
/// kept buffers.
const SHARED: usize = 256;

/// `f` with the buffers in `cell`, for a use of `size` vertices or pieces.
/// Without a cell, beyond [`SHARED`], or while an enclosing use holds them,
/// `f` gets new buffers.
pub(super) fn with<T: Default, R>(
    cell: Option<&RefCell<T>>,
    size: usize,
    f: impl FnOnce(&mut T) -> R,
) -> R {
    match cell
        .filter(|_| size <= SHARED)
        .and_then(|c| c.try_borrow_mut().ok())
    {
        Some(mut buffers) => f(&mut buffers),
        None => f(&mut T::default()),
    }
}

/// A kept vector, emptied for one use and charged as a new one: the meter
/// sees the capacity a new vector would have after the same reservations,
/// whatever capacity is kept.
pub(super) struct Reused<'a, T> {
    values: &'a mut Vec<T>,
    charged: usize,
}

impl<'a, T> Reused<'a, T> {
    pub(super) fn new(values: &'a mut Vec<T>) -> Self {
        values.clear();
        Self { values, charged: 0 }
    }

    /// As [`Meter::reserve`].
    pub(super) fn reserve(&mut self, bytes: &mut Meter, additional: usize) -> Result<()> {
        bytes.reserve(self.values, additional)?;
        self.charged = self.charged.max(self.values.len() + additional);
        Ok(())
    }

    /// As [`Meter::reserve_capacity`].
    pub(super) fn reserve_capacity(&mut self, bytes: &mut Meter, additional: usize) -> Result<()> {
        bytes.reserve_charged(self.values, &mut self.charged, additional)
    }
}

impl<T> Deref for Reused<'_, T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        self.values
    }
}

impl<T> DerefMut for Reused<'_, T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        self.values
    }
}
