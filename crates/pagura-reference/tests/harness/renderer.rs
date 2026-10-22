//! Runs `pagura-paint` and the reference on a recorded case.

use crate::cases::{Recorded, limits};
use pagura_core::{Limits, Result, Store};
use pagura_paint::{OverprintMode, RenderRequest, Scratch, prepare};
use pagura_raster::{DeviceRect, Pixmap, Rgba8};
use pagura_reference::{Overprint, Rendering, Request};

/// Bytes of the store each rendering gets.
const STORE_BYTES: usize = 64 << 20;

/// An empty store.
pub fn fresh() -> Store {
    Store::new(STORE_BYTES)
}

/// Whether charge probes share a filled store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cache {
    /// Every preparation starts from an empty store.
    Fresh,
    /// Preparations share a store a first preparation filled.
    Warm,
}

/// One rendering of a recorded case.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    /// The pixels.
    pub region: DeviceRect,
    /// Rows per band.
    pub band_height: u32,
}

impl Run {
    /// The whole page at the default band height.
    pub fn page(recorded: &Recorded) -> Self {
        Self {
            region: recorded.page,
            band_height: RenderRequest::BAND_HEIGHT,
        }
    }

    fn request(&self, recorded: &Recorded) -> RenderRequest {
        let mut request = RenderRequest::new(recorded.transform, recorded.page, self.region);
        request.background = Rgba8::WHITE;
        request.band_height = self.band_height;
        request.overprint = if recorded.simulate {
            OverprintMode::Simulate
        } else {
            OverprintMode::Ignore
        };
        request
    }

    /// Prepares the rendering under `limits` without executing it.
    ///
    /// # Errors
    ///
    /// Returns the preparation's error.
    pub fn prepare(&self, recorded: &Recorded, limits: &Limits, store: &Store) -> Result<()> {
        prepare(&recorded.scene, &self.request(recorded), store, limits).map(|_| ())
    }

    /// Renders with `pagura-paint` and a fresh store: premultiplied RGBA8
    /// over the region.
    ///
    /// # Errors
    ///
    /// Returns the preparation's error.
    pub fn render(&self, recorded: &Recorded) -> Result<Vec<u8>> {
        self.render_with(recorded, &fresh())
    }

    /// Renders with `pagura-paint` and `store`.
    ///
    /// # Errors
    ///
    /// Returns the preparation's error.
    pub fn render_with(&self, recorded: &Recorded, store: &Store) -> Result<Vec<u8>> {
        let limits = limits();
        let prepared = prepare(&recorded.scene, &self.request(recorded), store, &limits)?;
        let mut target = Pixmap::new(
            self.region.width(),
            self.region.height(),
            Rgba8::WHITE,
            limits.pixels,
        )?;
        prepared
            .with_scratch(&mut Scratch::default())?
            .render(&mut target);
        Ok(target.data().to_vec())
    }

    /// Renders with the reference.
    ///
    /// # Errors
    ///
    /// Returns the reference's error.
    pub fn reference(&self, recorded: &Recorded) -> Result<Rendering> {
        let mut request = Request::new(recorded.transform, self.region);
        request.overprint = if recorded.simulate {
            Overprint::Simulate
        } else {
            Overprint::Ignore
        };
        pagura_reference::render(&recorded.scene, &request, &limits())
    }

    /// The smallest value of the budget `field` selects at which
    /// preparation succeeds: the charge preparation reserves against it.
    /// With [`Cache::Fresh`] every preparation starts from an empty store;
    /// with [`Cache::Warm`] they share one that a first preparation filled.
    /// `None` when preparation fails even at the default.
    pub fn charge(
        &self,
        recorded: &Recorded,
        field: fn(&mut Limits) -> &mut usize,
        cache: Cache,
    ) -> Option<usize> {
        let mut limits = limits();
        let maximum = *field(&mut limits);
        let warm = fresh();
        self.prepare(recorded, &limits, &warm).ok()?;
        let (mut low, mut high) = (0, maximum);
        while low < high {
            let middle = low + (high - low) / 2;
            *field(&mut limits) = middle;
            let prepared = match cache {
                Cache::Fresh => self.prepare(recorded, &limits, &fresh()),
                Cache::Warm => self.prepare(recorded, &limits, &warm),
            };
            if prepared.is_ok() {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        Some(low)
    }
}
