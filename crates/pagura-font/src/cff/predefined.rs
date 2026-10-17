//! CFF identifiers checked against TN 5176 appendices A-C.
//!
//! These are format data, not glyph outlines or executable font programs.

pub(super) const EXPERT_STRINGS: &str = "exclamsmall Hungarumlautsmall dollaroldstyle dollarsuperior ampersandsmall Acutesmall parenleftsuperior parenrightsuperior twodotenleader onedotenleader zerooldstyle oneoldstyle twooldstyle threeoldstyle fouroldstyle fiveoldstyle sixoldstyle sevenoldstyle eightoldstyle nineoldstyle commasuperior threequartersemdash periodsuperior questionsmall asuperior bsuperior centsuperior dsuperior esuperior isuperior lsuperior msuperior nsuperior osuperior rsuperior ssuperior tsuperior ff ffi ffl parenleftinferior parenrightinferior Circumflexsmall hyphensuperior Gravesmall Asmall Bsmall Csmall Dsmall Esmall Fsmall Gsmall Hsmall Ismall Jsmall Ksmall Lsmall Msmall Nsmall Osmall Psmall Qsmall Rsmall Ssmall Tsmall Usmall Vsmall Wsmall Xsmall Ysmall Zsmall colonmonetary onefitted rupiah Tildesmall exclamdownsmall centoldstyle Lslashsmall Scaronsmall Zcaronsmall Dieresissmall Brevesmall Caronsmall Dotaccentsmall Macronsmall figuredash hypheninferior Ogoneksmall Ringsmall Cedillasmall questiondownsmall oneeighth threeeighths fiveeighths seveneighths onethird twothirds zerosuperior foursuperior fivesuperior sixsuperior sevensuperior eightsuperior ninesuperior zeroinferior oneinferior twoinferior threeinferior fourinferior fiveinferior sixinferior seveninferior eightinferior nineinferior centinferior dollarinferior periodinferior commainferior Agravesmall Aacutesmall Acircumflexsmall Atildesmall Adieresissmall Aringsmall AEsmall Ccedillasmall Egravesmall Eacutesmall Ecircumflexsmall Edieresissmall Igravesmall Iacutesmall Icircumflexsmall Idieresissmall Ethsmall Ntildesmall Ogravesmall Oacutesmall Ocircumflexsmall Otildesmall Odieresissmall OEsmall Oslashsmall Ugravesmall Uacutesmall Ucircumflexsmall Udieresissmall Yacutesmall Thornsmall Ydieresissmall 001.000 001.001 001.002 001.003 Black Bold Book Light Medium Regular Roman Semibold";

pub(super) const EXPERT: &[u16] = &[
    0, 1, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 13, 14, 15, 99, 239, 240, 241, 242,
    243, 244, 245, 246, 247, 248, 27, 28, 249, 250, 251, 252, 253, 254, 255, 256, 257, 258, 259,
    260, 261, 262, 263, 264, 265, 266, 109, 110, 267, 268, 269, 270, 271, 272, 273, 274, 275, 276,
    277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287, 288, 289, 290, 291, 292, 293, 294, 295,
    296, 297, 298, 299, 300, 301, 302, 303, 304, 305, 306, 307, 308, 309, 310, 311, 312, 313, 314,
    315, 316, 317, 318, 158, 155, 163, 319, 320, 321, 322, 323, 324, 325, 326, 150, 164, 169, 327,
    328, 329, 330, 331, 332, 333, 334, 335, 336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346,
    347, 348, 349, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362, 363, 364, 365,
    366, 367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378,
];
pub(super) const EXPERT_SUBSET: &[u16] = &[
    0, 1, 231, 232, 235, 236, 237, 238, 13, 14, 15, 99, 239, 240, 241, 242, 243, 244, 245, 246,
    247, 248, 27, 28, 249, 250, 251, 253, 254, 255, 256, 257, 258, 259, 260, 261, 262, 263, 264,
    265, 266, 109, 110, 267, 268, 269, 270, 272, 300, 301, 302, 305, 314, 315, 158, 155, 163, 320,
    321, 322, 323, 324, 325, 326, 150, 164, 169, 327, 328, 329, 330, 331, 332, 333, 334, 335, 336,
    337, 338, 339, 340, 341, 342, 343, 344, 345, 346,
];

pub(super) fn expert_sid(code: u8) -> Option<u16> {
    Some(match code {
        32 => 1,
        33 => 229,
        34 => 230,
        36..=43 => u16::from(code) + 195,
        44..=46 | 58..=59 => u16::from(code) - 31,
        47 => 99,
        48..=57 => u16::from(code) + 191,
        60..=63 => u16::from(code) + 189,
        65..=69 => u16::from(code) + 188,
        73 => 258,
        76..=79 => u16::from(code) + 183,
        82..=84 => u16::from(code) + 181,
        86 => 266,
        87 => 109,
        88 => 110,
        89..=91 => u16::from(code) + 178,
        // TN 5176 Appendix B leaves code 92 unencoded.
        93..=126 => u16::from(code) + 177,
        161..=163 => u16::from(code) + 143,
        166..=170 => u16::from(code) + 141,
        172 => 312,
        175 => 313,
        178 => 314,
        179 => 315,
        182..=184 => u16::from(code) + 134,
        188 => 158,
        189 => 155,
        190 => 163,
        191..=197 => u16::from(code) + 128,
        200 => 326,
        201 => 150,
        202 => 164,
        203 => 169,
        204..=255 => u16::from(code) + 123,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{EXPERT, EXPERT_STRINGS, EXPERT_SUBSET, expert_sid};
    #[test]
    fn canonical_identifier_tables_are_consistent() {
        assert_eq!(EXPERT_STRINGS.split_ascii_whitespace().count(), 162);
        assert_eq!(EXPERT.len(), 166);
        assert_eq!(EXPERT_SUBSET.len(), 87);
        assert!(EXPERT_SUBSET.iter().all(|sid| EXPERT.contains(sid)));
        assert!(
            (0..=255)
                .filter_map(expert_sid)
                .all(|sid| EXPERT.contains(&sid))
        );
    }
}
