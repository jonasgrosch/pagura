//! Typed access to values that may be indirect (7.3.10).
//!
//! Each accessor resolves references within [`Limits::depth`](pagura_core::Limits)
//! and allocates nothing: an array is read into a fixed-size result. A value
//! of the wrong type is a syntax error in the object that held it.

use crate::Document;
use pagura_core::{Dictionary, Error, Name, ObjectId, Result, Value};

impl Document {
    /// Resolves `value` and requires an integer.
    ///
    /// # Errors
    ///
    /// As [`Document::resolve`], plus a syntax error for another type.
    pub fn integer(&self, value: &Value) -> Result<i64> {
        let (value, origin) = self.located(value)?;
        value
            .as_integer()
            .ok_or_else(|| Error::syntax("expected an integer").in_object_opt(origin))
    }

    /// Resolves `value` and requires a finite number, integer or real.
    ///
    /// # Errors
    ///
    /// As [`Document::resolve`], plus a syntax error for another type or a
    /// non-finite real.
    pub fn number(&self, value: &Value) -> Result<f64> {
        let (value, origin) = self.located(value)?;
        number(value, origin)
    }

    /// Resolves `value` and requires a name.
    ///
    /// # Errors
    ///
    /// As [`Document::resolve`], plus a syntax error for another type.
    pub fn name<'a>(&'a self, value: &'a Value) -> Result<&'a Name> {
        let (value, origin) = self.located(value)?;
        value
            .as_name()
            .ok_or_else(|| Error::syntax("expected a name").in_object_opt(origin))
    }

    /// Resolves `value` to an array of exactly `N` numbers, resolving each
    /// element, as for a rectangle (`N` = 4, 7.9.5) or a matrix (`N` = 6).
    ///
    /// # Errors
    ///
    /// As [`Document::number`], plus a syntax error for a value that is not
    /// an array of `N` elements.
    pub fn numbers<const N: usize>(&self, value: &Value) -> Result<[f64; N]> {
        let (value, origin) = self.located(value)?;
        let elements = value
            .as_array()
            .filter(|elements| elements.len() == N)
            .ok_or_else(|| {
                Error::syntax("expected an array of a fixed number of numbers")
                    .in_object_opt(origin)
            })?;
        let mut numbers = [0.0; N];
        for (target, element) in numbers.iter_mut().zip(elements) {
            let (element, inner) = self.located(element)?;
            *target = number(element, inner.or(origin))?;
        }
        Ok(numbers)
    }

    /// The entry `key` of `dictionary`, resolved. Absent and null entries are
    /// `None` (7.3.9), as is a reference to a free or undefined object
    /// (7.3.10).
    ///
    /// # Errors
    ///
    /// As [`Document::resolve`].
    pub fn optional<'a>(
        &'a self,
        dictionary: &'a Dictionary,
        key: &[u8],
    ) -> Result<Option<&'a Value>> {
        let Some(value) = dictionary.get(key) else {
            return Ok(None);
        };
        let value = self.resolve(value)?;
        Ok((!value.is_null()).then_some(value))
    }

    fn located<'a>(&'a self, value: &'a Value) -> Result<(&'a Value, Option<ObjectId>)> {
        self.resolve_inner(value, &mut Vec::new())
    }
}

fn number(value: &Value, origin: Option<ObjectId>) -> Result<f64> {
    value
        .as_number()
        .filter(|number| number.is_finite())
        .ok_or_else(|| Error::syntax("expected a finite number").in_object_opt(origin))
}

#[cfg(test)]
mod tests {
    use crate::{Document, OpenOptions};
    use pagura_core::{Dictionary, ErrorKind, Name, ObjectId, Value};
    use pagura_testkit::fixture;

    fn document() -> Document {
        Document::open(
            fixture("document", "xref-stream.pdf"),
            &OpenOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn typed_accessors_resolve_references_and_name_the_holding_object() {
        let document = document();
        let catalog = ObjectId::new(1, 0);
        let reference = Value::Reference(catalog);
        assert_eq!(document.integer(&Value::Integer(-3)).unwrap(), -3);
        assert_eq!(document.number(&Value::Integer(2)).unwrap(), 2.0);
        assert_eq!(document.number(&Value::Real(0.5)).unwrap(), 0.5);
        let error = document.integer(&reference).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.object, Some(catalog));
        assert_eq!(
            document.name(&Value::Name(Name::new(b"Catalog"))).unwrap(),
            b"Catalog"
        );
        assert_eq!(
            document.number(&Value::Real(f64::NAN)).unwrap_err().kind,
            ErrorKind::Syntax
        );
        // A reference to an undefined object is null (7.3.10), not a number.
        let missing = Value::Reference(ObjectId::new(999, 0));
        assert_eq!(
            document.number(&missing).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }

    #[test]
    fn fixed_arrays_need_exactly_n_numbers_and_optional_entries_skip_null() {
        let document = document();
        let numbers = |values: Vec<Value>| Value::Array(values);
        assert_eq!(
            document
                .numbers::<4>(&numbers(vec![
                    Value::Integer(0),
                    Value::Real(1.5),
                    Value::Integer(-2),
                    Value::Integer(3)
                ]))
                .unwrap(),
            [0.0, 1.5, -2.0, 3.0]
        );
        for wrong in [
            numbers(vec![Value::Integer(0); 3]),
            numbers(vec![Value::Integer(0); 5]),
            numbers(vec![Value::Null; 4]),
            Value::Integer(4),
        ] {
            assert_eq!(
                document.numbers::<4>(&wrong).unwrap_err().kind,
                ErrorKind::Syntax
            );
        }
        let mut dictionary = Dictionary::new();
        dictionary.insert(Name::new(b"Null"), Value::Null);
        dictionary.insert(Name::new(b"Free"), Value::Reference(ObjectId::new(999, 0)));
        dictionary.insert(Name::new(b"Value"), Value::Integer(7));
        assert_eq!(document.optional(&dictionary, b"Absent").unwrap(), None);
        assert_eq!(document.optional(&dictionary, b"Null").unwrap(), None);
        assert_eq!(document.optional(&dictionary, b"Free").unwrap(), None);
        assert_eq!(
            document.optional(&dictionary, b"Value").unwrap(),
            Some(&Value::Integer(7))
        );
    }
}
