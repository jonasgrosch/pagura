//! Identifiers (X.690 8.1.2) and the universal types this crate reads
//! (X.680 8.6 Table 1).

/// Identifier class (X.690 8.1.2.2).
///
/// Two identifier bits encode the class, so X.690 closes the set and the
/// enum is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// ASN.1 universal type.
    Universal,
    /// Application-defined type.
    Application,
    /// Schema-defined context tag.
    Context,
    /// Private type.
    Private,
}

/// Whether an encoding's content is a value or nested encodings
/// (X.690 8.1.2.5).
///
/// One identifier bit encodes the form, so X.690 closes the set and the enum
/// is exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// The content octets are the value.
    Primitive,
    /// The content octets are complete nested encodings.
    Constructed,
}

/// An ASN.1 identifier.
///
/// Every class, form and number is encodable, so the fields carry no
/// invariant and are public; X.690 8.1.2 closes the record, so it is
/// exhaustive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag {
    /// Identifier class.
    pub class: Class,
    /// Primitive or constructed encoding.
    pub form: Form,
    /// Tag number; numbers exceeding `u32` are a `Limit` error.
    pub number: u32,
}

impl Tag {
    /// Creates an identifier.
    pub const fn new(class: Class, form: Form, number: u32) -> Self {
        Self {
            class,
            form,
            number,
        }
    }

    /// Creates a universal identifier.
    pub const fn universal(number: u32, form: Form) -> Self {
        Self::new(Class::Universal, form, number)
    }

    /// Creates a context-specific identifier, as a schema's `[n]` tag.
    pub const fn context(number: u32, form: Form) -> Self {
        Self::new(Class::Context, form, number)
    }

    /// Whether the content consists of nested encodings.
    pub const fn is_constructed(self) -> bool {
        matches!(self.form, Form::Constructed)
    }

    /// Whether this is universal type `number`, in either form.
    pub(crate) const fn is_universal(self, number: u32) -> bool {
        matches!(self.class, Class::Universal) && self.number == number
    }
}

/// Universal tag numbers (X.680 8.6 Table 1).
pub(crate) mod universal {
    pub(crate) const END_OF_CONTENTS: u32 = 0;
    pub(crate) const BOOLEAN: u32 = 1;
    pub(crate) const INTEGER: u32 = 2;
    pub(crate) const BIT_STRING: u32 = 3;
    pub(crate) const OCTET_STRING: u32 = 4;
    pub(crate) const NULL: u32 = 5;
    pub(crate) const OBJECT_IDENTIFIER: u32 = 6;
    pub(crate) const ENUMERATED: u32 = 10;
    pub(crate) const UTF8_STRING: u32 = 12;
    pub(crate) const RELATIVE_OID: u32 = 13;
    pub(crate) const SEQUENCE: u32 = 16;
    pub(crate) const SET: u32 = 17;
    pub(crate) const NUMERIC_STRING: u32 = 18;
    pub(crate) const PRINTABLE_STRING: u32 = 19;
    pub(crate) const TELETEX_STRING: u32 = 20;
    pub(crate) const VIDEOTEX_STRING: u32 = 21;
    pub(crate) const IA5_STRING: u32 = 22;
    pub(crate) const UTC_TIME: u32 = 23;
    pub(crate) const GENERALIZED_TIME: u32 = 24;
    pub(crate) const GRAPHIC_STRING: u32 = 25;
    pub(crate) const VISIBLE_STRING: u32 = 26;
    pub(crate) const GENERAL_STRING: u32 = 27;
    pub(crate) const UNIVERSAL_STRING: u32 = 28;
    pub(crate) const BMP_STRING: u32 = 30;

    /// Whether `number` is a character string type this crate reads.
    pub(crate) const fn is_character_string(number: u32) -> bool {
        matches!(
            number,
            UTF8_STRING
                | NUMERIC_STRING
                | PRINTABLE_STRING
                | TELETEX_STRING
                | VIDEOTEX_STRING
                | IA5_STRING
                | GRAPHIC_STRING
                | VISIBLE_STRING
                | GENERAL_STRING
                | UNIVERSAL_STRING
                | BMP_STRING
        )
    }
}

const fn primitive(number: u32) -> Tag {
    Tag::universal(number, Form::Primitive)
}

/// Universal BOOLEAN.
pub const BOOLEAN: Tag = primitive(universal::BOOLEAN);
/// Universal INTEGER.
pub const INTEGER: Tag = primitive(universal::INTEGER);
/// Universal primitive BIT STRING.
pub const BIT_STRING: Tag = primitive(universal::BIT_STRING);
/// Universal primitive OCTET STRING; BER may also use the constructed form.
pub const OCTET_STRING: Tag = primitive(universal::OCTET_STRING);
/// Universal NULL.
pub const NULL: Tag = primitive(universal::NULL);
/// Universal OBJECT IDENTIFIER.
pub const OBJECT_IDENTIFIER: Tag = primitive(universal::OBJECT_IDENTIFIER);
/// Universal ENUMERATED.
pub const ENUMERATED: Tag = primitive(universal::ENUMERATED);
/// Universal UTF8String.
pub const UTF8_STRING: Tag = primitive(universal::UTF8_STRING);
/// Universal RELATIVE-OID.
pub const RELATIVE_OID: Tag = primitive(universal::RELATIVE_OID);
/// Universal SEQUENCE or SEQUENCE OF.
pub const SEQUENCE: Tag = Tag::universal(universal::SEQUENCE, Form::Constructed);
/// Universal SET or SET OF; its schema decides which ordering applies.
pub const SET: Tag = Tag::universal(universal::SET, Form::Constructed);
/// Universal NumericString.
pub const NUMERIC_STRING: Tag = primitive(universal::NUMERIC_STRING);
/// Universal PrintableString.
pub const PRINTABLE_STRING: Tag = primitive(universal::PRINTABLE_STRING);
/// Universal TeletexString (T61String).
pub const TELETEX_STRING: Tag = primitive(universal::TELETEX_STRING);
/// Universal VideotexString.
pub const VIDEOTEX_STRING: Tag = primitive(universal::VIDEOTEX_STRING);
/// Universal IA5String.
pub const IA5_STRING: Tag = primitive(universal::IA5_STRING);
/// Universal UTCTime.
pub const UTC_TIME: Tag = primitive(universal::UTC_TIME);
/// Universal GeneralizedTime.
pub const GENERALIZED_TIME: Tag = primitive(universal::GENERALIZED_TIME);
/// Universal GraphicString.
pub const GRAPHIC_STRING: Tag = primitive(universal::GRAPHIC_STRING);
/// Universal VisibleString.
pub const VISIBLE_STRING: Tag = primitive(universal::VISIBLE_STRING);
/// Universal GeneralString.
pub const GENERAL_STRING: Tag = primitive(universal::GENERAL_STRING);
/// Universal UniversalString.
pub const UNIVERSAL_STRING: Tag = primitive(universal::UNIVERSAL_STRING);
/// Universal BMPString.
pub const BMP_STRING: Tag = primitive(universal::BMP_STRING);
