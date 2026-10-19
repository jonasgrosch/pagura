//! Bounded static procedure aliases (Type 1 2.4, 7.3 and 10.3).
//! Definitions are matched, never executed or expanded as PostScript.
use super::tokens::Tokens;
use crate::features;
use crate::read::unsupported;
use pagura_core::{Name, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Assignment {
    Define,
    Put,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reader,
    Assignment(Assignment),
}

#[derive(Debug, Default)]
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

    pub(super) fn reader(&self, word: &[u8]) -> bool {
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

    pub(super) fn finish(&self, r: &mut Tokens<'_, '_, '_>, assignment: Assignment) -> Result<()> {
        let mut word = r.required()?;
        if word == b"noaccess" {
            word = r.required()?;
        }
        if self.assignment(word, assignment) {
            Ok(())
        } else {
            Err(computed(r, "computed Type 1 assignment is not implemented"))
        }
    }

    pub(super) fn definition(&mut self, token: &[u8], r: &mut Tokens<'_, '_, '_>) -> Result<()> {
        let Some(name) = token.strip_prefix(b"/") else {
            return Ok(());
        };
        let known = self.kind(name).is_some();
        let Some(body) = r.procedure()? else {
            return if known {
                Err(computed(r, "computed Type 1 procedure is not implemented"))
            } else {
                Ok(())
            };
        };
        let Some(kind) = classify(Tokens::new(body, r.work()))? else {
            return if known {
                Err(computed(r, "nonstatic Type 1 procedure is not implemented"))
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
            return Err(computed(
                r,
                "computed Type 1 procedure definition is not implemented",
            ));
        }
        if !self.0.contains_key(name) {
            let at = r.position();
            r.work().entries(1, at)?;
        }
        let name = Name::try_new(name).map_err(|e| e.at(r.position()))?;
        self.0.insert(name, kind);
        Ok(())
    }
}

fn computed(r: &Tokens<'_, '_, '_>, message: &'static str) -> pagura_core::Error {
    unsupported(features::TYPE1_POSTSCRIPT, r.position(), message)
}

fn classify(mut body: Tokens<'_, '_, '_>) -> Result<Option<Kind>> {
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
