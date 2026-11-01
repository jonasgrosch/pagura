//! Colour component values, stored inline for up to four components.

use crate::space::MAX_COMPONENTS;

/// The component values of one colour: at most [`MAX_COMPONENTS`], inline up
/// to four so that device colours allocate nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct Components(Repr);

#[derive(Debug, Clone, PartialEq)]
enum Repr {
    Inline { len: u8, values: [f64; 4] },
    Heap(Box<[f64]>),
}

impl Components {
    /// Components copied from `values`; values beyond [`MAX_COMPONENTS`]
    /// are dropped, which no validated colour space produces.
    pub fn from_slice(values: &[f64]) -> Self {
        let values = &values[..values.len().min(MAX_COMPONENTS)];
        if let Ok(len) = u8::try_from(values.len())
            && values.len() <= 4
        {
            let mut inline = [0.0; 4];
            inline[..values.len()].copy_from_slice(values);
            return Self(Repr::Inline {
                len,
                values: inline,
            });
        }
        Self(Repr::Heap(values.into()))
    }

    /// `count` components of `value`.
    pub fn filled(value: f64, count: usize) -> Self {
        let mut values = [0.0; MAX_COMPONENTS];
        let count = count.min(MAX_COMPONENTS);
        values[..count].fill(value);
        Self::from_slice(&values[..count])
    }

    /// The values.
    pub fn as_slice(&self) -> &[f64] {
        match &self.0 {
            Repr::Inline { len, values } => &values[..usize::from(*len)],
            Repr::Heap(values) => values,
        }
    }

    /// Bytes owned by the component buffer, excluding inline storage.
    pub fn heap_bytes(&self) -> usize {
        match &self.0 {
            Repr::Inline { .. } => 0,
            Repr::Heap(values) => size_of_val(&**values),
        }
    }

    /// The number of components.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Whether there are no components (a coloured pattern's colour).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::ops::Deref for Components {
    type Target = [f64];

    fn deref(&self) -> &[f64] {
        self.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_colours_are_inline_and_long_ones_kept() {
        let short = Components::from_slice(&[0.5, 1.0]);
        assert_eq!(short.as_slice(), &[0.5, 1.0]);
        assert_eq!(short.heap_bytes(), 0);
        let six = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let long = Components::from_slice(&six);
        assert_eq!(&*long, &six);
        assert_eq!(long.heap_bytes(), size_of_val(&six));
        assert_eq!(Components::filled(1.0, 3).as_slice(), &[1.0; 3]);
        assert!(Components::from_slice(&[]).is_empty());
    }
}
