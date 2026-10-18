//! Bounded static procedure aliases (Type 1 2.4, 7.3 and 10.3).
//! Definitions are matched, never executed or expanded as PostScript.
use super::{Tokens, unsupported};
use crate::read::cap;
use pagura_core::{Limits, Name, Result};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Assignment {
    Define,
    Put,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reader,
    Assignment(Assignment),
}
#[derive(Default)]
pub(super) struct Aliases(BTreeMap<Name, Kind>);

impl Aliases {
    fn kind(&self, word: &[u8]) -> Option<Kind> {
        self.0.get(word).copied().or(match word {
            b"RD" | b"-|" => Some(Kind::Reader),
            b"ND" | b"|-" => Some(Kind::Assignment(Assignment::Define)),
            b"NP" | b"|" => Some(Kind::Assignment(Assignment::Put)),
            _ => None,
        })
    }
    pub fn reader(&self, word: &[u8]) -> bool {
        self.kind(word) == Some(Kind::Reader)
    }
    fn assignment(&self, word: &[u8], assignment: Assignment) -> bool {
        self.kind(word) == Some(Kind::Assignment(assignment))
            || word
                == match assignment {
                    Assignment::Define => b"def",
                    Assignment::Put => b"put",
                }
    }
    pub fn finish(&self, r: &mut Tokens<'_>, assignment: Assignment) -> Result<()> {
        let mut word = r.required()?;
        if word == b"noaccess" {
            word = r.required()?;
        }
        if self.assignment(word, assignment) {
            Ok(())
        } else {
            Err(unsupported(
                r.pos,
                "computed Type 1 assignment is not implemented",
            ))
        }
    }
    pub fn definition(&mut self, token: &[u8], r: &mut Tokens<'_>, limits: &Limits) -> Result<()> {
        let Some(name) = token.strip_prefix(b"/") else {
            return Ok(());
        };
        let known = self.kind(name).is_some();
        let Some(body) = r.procedure()? else {
            return if known {
                Err(unsupported(
                    r.pos,
                    "computed Type 1 procedure is not implemented",
                ))
            } else {
                Ok(())
            };
        };
        let Some(kind) = classify(Tokens::new(body, limits))? else {
            return if known {
                Err(unsupported(
                    r.pos,
                    "nonstatic Type 1 procedure is not implemented",
                ))
            } else {
                Ok(())
            };
        };
        let mut end = r.required()?;
        if end == b"bind" {
            end = r.required()?;
        }
        if matches!(end, b"executeonly" | b"readonly" | b"noaccess") {
            end = r.required()?;
        }
        if !self.assignment(end, Assignment::Define) {
            return Err(unsupported(
                r.pos,
                "computed Type 1 procedure definition is not implemented",
            ));
        }
        cap(
            self.0
                .len()
                .saturating_add(usize::from(!self.0.contains_key(name))),
            limits
                .objects
                .min(limits.value_nodes)
                .min(limits.decoded_bytes / (size_of::<Name>() + 64)),
            r.pos,
        )?;
        self.0.insert(Name::new(name), kind);
        Ok(())
    }
}

fn classify(mut body: Tokens<'_>) -> Result<Option<Kind>> {
    let mut word = body.raw()?;
    let kind = if word == Some(b"string".as_slice()) {
        for expected in [b"currentfile".as_slice(), b"exch", b"readstring", b"pop"] {
            if body.raw()? != Some(expected) {
                return Ok(None);
            }
        }
        Kind::Reader
    } else {
        if word == Some(b"noaccess".as_slice()) {
            word = body.raw()?;
        }
        match word {
            Some(b"def") => Kind::Assignment(Assignment::Define),
            Some(b"put") => Kind::Assignment(Assignment::Put),
            _ => return Ok(None),
        }
    };
    Ok(if body.raw()?.is_none() {
        Some(kind)
    } else {
        None
    })
}
