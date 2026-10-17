//! Shading patterns anchored to their parent stream's initial user space (8.7.2).

use crate::shading::{Shading, numbers};
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;
use pagura_raster::Transform;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct Pattern {
    pub(crate) shading: Arc<Shading>,
    pub(crate) transform: Transform,
}
impl Pattern {
    pub(crate) fn parse(
        document: &Document,
        resources: &Dictionary,
        name: &[u8],
        initial: Transform,
    ) -> Result<Self> {
        let value = resources
            .get(b"Pattern")
            .map(|v| document.resolve_dictionary(v))
            .transpose()?
            .and_then(|d| d.get(name))
            .ok_or(Error::syntax("undefined pattern resource"))?;
        let d = document.resolve_dictionary(value)?;
        let kind = d
            .get(b"PatternType")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("pattern requires PatternType"))?;
        if kind == 1 {
            return Err(Error::unsupported(
                crate::features::PATTERN_TILING,
                "tiling patterns are not implemented",
            ));
        }
        if kind != 2 {
            return Err(Error::syntax("invalid PatternType"));
        }
        if let Some(state) = d.get(b"ExtGState")
            && !document.resolve_dictionary(state)?.is_empty()
        {
            return Err(Error::unsupported(
                crate::features::PATTERN_STATE,
                "pattern rendering parameters are not implemented",
            ));
        }
        let transform = d
            .get(b"Matrix")
            .map(|v| {
                let m = numbers::<6>(document, v)?;
                Ok(Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
            })
            .transpose()?
            .unwrap_or(Transform::IDENTITY)
            .then(initial);
        if !transform.is_finite() {
            return Err(Error::limit("pattern transform overflow"));
        }
        let shading = Shading::parse(
            document,
            resources,
            d.get(b"Shading")
                .ok_or(Error::syntax("pattern requires Shading"))?,
        )?;
        Ok(Self {
            shading: Arc::new(shading),
            transform,
        })
    }
}
