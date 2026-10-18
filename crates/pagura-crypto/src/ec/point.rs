//! Jacobian coordinates: affine (X/Z², Y/Z³), with Z=0 for infinity.
//! The short-Weierstrass group law is evaluated by clearing denominators.
//! Each curve has a=-3 (SP 800-186 sections 3.2.1.3–3.2.1.5).

use super::{Parameters, Uint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Point {
    pub(crate) x: Uint,
    pub(crate) y: Uint,
    pub(crate) z: Uint,
}

impl Point {
    pub(crate) const IDENTITY: Self = Self {
        x: Uint::ZERO,
        y: Uint::ONE,
        z: Uint::ZERO,
    };

    pub(crate) fn affine(x: Uint, y: Uint, parameters: &Parameters) -> Self {
        Self {
            x,
            y,
            z: parameters.field.one,
        }
    }

    pub(crate) fn double(self, parameters: &Parameters) -> Self {
        if self.z.is_zero() || self.y.is_zero() {
            return Self::IDENTITY;
        }
        let f = &parameters.field;
        let delta = f.square(self.z);
        let gamma = f.square(self.y);
        let beta = f.mul(self.x, gamma);
        let slope = f.mul(f.sub(self.x, delta), f.add(self.x, delta));
        let slope = f.add(f.add(slope, slope), slope);
        let beta2 = f.add(beta, beta);
        let beta4 = f.add(beta2, beta2);
        let beta8 = f.add(beta4, beta4);
        let x = f.sub(f.square(slope), beta8);
        let gamma_squared = f.square(gamma);
        let twice_gamma_squared = f.add(gamma_squared, gamma_squared);
        let four_gamma_squared = f.add(twice_gamma_squared, twice_gamma_squared);
        let eight_gamma_squared = f.add(four_gamma_squared, four_gamma_squared);
        let y = f.sub(f.mul(slope, f.sub(beta4, x)), eight_gamma_squared);
        let z = f.sub(f.sub(f.square(f.add(self.y, self.z)), gamma), delta);
        Self { x, y, z }
    }

    pub(crate) fn add(self, other: Self, parameters: &Parameters) -> Self {
        if self.z.is_zero() {
            return other;
        }
        if other.z.is_zero() {
            return self;
        }
        let f = &parameters.field;
        let z1_squared = f.square(self.z);
        let z2_squared = f.square(other.z);
        let u1 = f.mul(self.x, z2_squared);
        let u2 = f.mul(other.x, z1_squared);
        let s1 = f.mul(self.y, f.mul(other.z, z2_squared));
        let s2 = f.mul(other.y, f.mul(self.z, z1_squared));
        let h = f.sub(u2, u1);
        let r = f.sub(s2, s1);
        if h.is_zero() {
            return if r.is_zero() {
                self.double(parameters)
            } else {
                Self::IDENTITY
            };
        }
        let h_squared = f.square(h);
        let h_cubed = f.mul(h_squared, h);
        let u1_h_squared = f.mul(u1, h_squared);
        let x = f.sub(
            f.sub(f.square(r), h_cubed),
            f.add(u1_h_squared, u1_h_squared),
        );
        let y = f.sub(f.mul(r, f.sub(u1_h_squared, x)), f.mul(s1, h_cubed));
        let z = f.mul(h, f.mul(self.z, other.z));
        Self { x, y, z }
    }

    pub(crate) fn affine_coordinates(self, parameters: &Parameters) -> Option<(Uint, Uint)> {
        let f = &parameters.field;
        let inverse = f.inverse(self.z)?;
        let inverse_squared = f.square(inverse);
        Some((
            f.decode(f.mul(self.x, inverse_squared)),
            f.decode(f.mul(self.y, f.mul(inverse, inverse_squared))),
        ))
    }

    /// Shamir's trick uses one doubling per scalar bit, for public scalars.
    pub(crate) fn double_scalar(
        self,
        u: Uint,
        v: Uint,
        parameters: &Parameters,
        bits: usize,
    ) -> Self {
        let generator = parameters.generator();
        let table = [
            Self::IDENTITY,
            generator,
            self,
            generator.add(self, parameters),
        ];
        let mut result = Self::IDENTITY;
        for i in (0..bits).rev() {
            result = result.double(parameters);
            let index = usize::from(u.bit(i)) + 2 * usize::from(v.bit(i));
            if index != 0 {
                result = result.add(table[index], parameters);
            }
        }
        result
    }
}
