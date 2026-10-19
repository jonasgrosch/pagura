//! Live compositor memory: every holding returns its bytes when dropped.

use pagura_core::{Error, Meter, Result};
use std::cell::RefCell;
use std::rc::Rc;

/// Bytes the compositor holds live: group and page surfaces, the cached soft
/// mask, glyph geometry in use, image sampling buffers and transfer regions.
/// The raster and glyph caches yield to these holdings
/// (`Context::trim_raster`), and raster calls receive what remains.
#[derive(Clone)]
pub(super) struct Memory(Rc<RefCell<Meter>>);

impl Memory {
    pub(super) fn new(maximum: usize) -> Self {
        Self(Rc::new(RefCell::new(Meter::new(
            maximum,
            "compositor memory budget exceeded",
        ))))
    }
    /// Charges `bytes` until the holding drops; `message` names what would
    /// exceed the budget.
    pub(super) fn hold(&self, bytes: usize, message: &'static str) -> Result<Held> {
        self.0
            .borrow_mut()
            .charge(bytes)
            .map_err(|_| Error::limit(message))?;
        Ok(Held {
            meter: Rc::clone(&self.0),
            bytes,
        })
    }
    pub(super) fn used(&self) -> usize {
        self.0.borrow().used()
    }
    pub(super) fn remaining(&self) -> usize {
        self.0.borrow().remaining()
    }
    pub(super) fn maximum(&self) -> usize {
        self.0.borrow().max()
    }
}

/// Bytes charged to [`Memory`] for as long as the holding lives.
pub(super) struct Held {
    meter: Rc<RefCell<Meter>>,
    bytes: usize,
}

impl Held {
    /// Charges `bytes` more, released with the rest.
    pub(super) fn add(&mut self, bytes: usize, message: &'static str) -> Result<()> {
        self.meter
            .borrow_mut()
            .charge(bytes)
            .map_err(|_| Error::limit(message))?;
        self.bytes += bytes;
        Ok(())
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.meter.borrow_mut().refund(self.bytes);
    }
}
