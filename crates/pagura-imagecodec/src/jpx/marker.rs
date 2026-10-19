//! Codestream markers, T.800 A.1 and Table A.2.

/// A marker: 0xFF followed by a code byte (T.800 A.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Marker(u16);

impl Marker {
    /// Start of codestream (T.800 A.4.1).
    pub(super) const SOC: Self = Self(0xff4f);
    /// Start of tile-part (T.800 A.4.2).
    pub(super) const SOT: Self = Self(0xff90);
    /// Start of data (T.800 A.4.3).
    pub(super) const SOD: Self = Self(0xff93);
    /// End of codestream (T.800 A.4.4).
    pub(super) const EOC: Self = Self(0xffd9);
    /// Image and tile size (T.800 A.5.1).
    pub(super) const SIZ: Self = Self(0xff51);
    /// Coding style default (T.800 A.6.1).
    pub(super) const COD: Self = Self(0xff52);
    /// Coding style component (T.800 A.6.2).
    pub(super) const COC: Self = Self(0xff53);
    /// Region of interest (T.800 A.6.3).
    pub(super) const RGN: Self = Self(0xff5e);
    /// Quantization default (T.800 A.6.4).
    pub(super) const QCD: Self = Self(0xff5c);
    /// Quantization component (T.800 A.6.5).
    pub(super) const QCC: Self = Self(0xff5d);
    /// Progression order change (T.800 A.6.6).
    pub(super) const POC: Self = Self(0xff5f);
    /// Tile-part lengths (T.800 A.7.1).
    pub(super) const TLM: Self = Self(0xff55);
    /// Packet lengths in the main header (T.800 A.7.2).
    pub(super) const PLM: Self = Self(0xff57);
    /// Packet lengths in a tile-part header (T.800 A.7.3).
    pub(super) const PLT: Self = Self(0xff58);
    /// Packed packet headers in the main header (T.800 A.7.4).
    pub(super) const PPM: Self = Self(0xff60);
    /// Packed packet headers in a tile-part header (T.800 A.7.5).
    pub(super) const PPT: Self = Self(0xff61);
    /// Start of packet (T.800 A.8.1).
    pub(super) const SOP: Self = Self(0xff91);
    /// End of packet header (T.800 A.8.2).
    pub(super) const EPH: Self = Self(0xff92);
    /// Component registration (T.800 A.9.1).
    pub(super) const CRG: Self = Self(0xff63);
    /// Comment (T.800 A.9.2).
    pub(super) const COM: Self = Self(0xff64);

    /// The marker whose second byte is `code`.
    pub(super) const fn new(code: u8) -> Self {
        Self(u16::from_be_bytes([0xff, code]))
    }

    /// The marker's two bytes.
    pub(super) const fn bytes(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Whether this is one of the reserved markers without parameters,
    /// 0xFF30 to 0xFF3F, which decoders skip (T.800 A.1.3).
    pub(super) const fn is_reserved(self) -> bool {
        matches!(self.0, 0xff30..=0xff3f)
    }
}
