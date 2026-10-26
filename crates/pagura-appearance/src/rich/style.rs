//! Inherited XFA 3.3 chapter 27 character and paragraph properties.

use pagura_core::{Error, Name, Result};
use pagura_document::forms::Justification;

#[derive(Clone, Debug)]
pub(super) struct Style {
    pub(super) family: Name,
    pub(super) size: f64,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) color: Option<[f64; 3]>,
    pub(super) rise: f64,
    pub(super) scale: [f64; 2],
    pub(super) leading: Option<f64>,
    pub(super) spacing: Spacing,
    pub(super) decoration: u8,
    pub(super) alignment: Justification,
    pub(super) margin: [f64; 4],
    pub(super) indent: f64,
    pub(super) preserve: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Spacing {
    Absolute(f64),
    Em(f64),
    Space(f64),
}

impl Style {
    pub(super) fn new(family: Name, size: f64, alignment: Justification) -> Self {
        Self {
            family,
            size,
            bold: false,
            italic: false,
            color: None,
            rise: 0.0,
            scale: [1.0; 2],
            leading: None,
            spacing: Spacing::Absolute(0.0),
            decoration: 0,
            alignment,
            margin: [0.0; 4],
            indent: 0.0,
            preserve: false,
        }
    }

    pub(super) fn apply(&mut self, css: &str) -> Result<()> {
        for declaration in css
            .split(';')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            let (property, value) = declaration
                .split_once(':')
                .ok_or(Error::syntax("invalid rich text style declaration"))?;
            let value = value.trim();
            match property.trim() {
                "font-family" => self.family = family(value)?,
                "font-size" => {
                    self.size = absolute_length(value)?;
                    if self.size <= 0.0 {
                        return Err(Error::syntax("rich text font size must be positive"));
                    }
                }
                "font-weight" => self.bold = weight(value)?,
                "font-style" => self.italic = italic(value)?,
                "font" => self.font(value)?,
                "color" => self.color = Some(color(value)?),
                "text-align" => {
                    self.alignment = match value {
                        "left" => Justification::Left,
                        "center" => Justification::Center,
                        "right" => Justification::Right,
                        _ => return Err(super::unsupported("rich text alignment is unavailable")),
                    }
                }
                "vertical-align" => {
                    self.rise = if value == "baseline" {
                        0.0
                    } else {
                        absolute_length(value)?
                    }
                }
                "line-height" => {
                    self.leading = if value == "normal" {
                        None
                    } else {
                        Some(nonnegative(absolute_length(value)?)?)
                    }
                }
                "letter-spacing" => {
                    self.spacing = if value == "normal" {
                        Spacing::Absolute(0.0)
                    } else if value.ends_with('%') {
                        Spacing::Space(length(value)?)
                    } else if value.ends_with("em") {
                        Spacing::Em(length(value)?)
                    } else {
                        Spacing::Absolute(length(value)?)
                    }
                }
                "text-decoration" => self.decoration = decoration(value)?,
                "xfa-font-horizontal-scale" => self.scale[0] = percentage(value)?,
                "xfa-font-vertical-scale" => self.scale[1] = percentage(value)?,
                "xfa-spacerun" => {
                    self.preserve = match value {
                        "yes" => true,
                        "no" => false,
                        _ => return Err(Error::syntax("invalid rich text whitespace mode")),
                    }
                }
                "margin-top" => self.margin[0] = absolute_length(value)?,
                "margin-right" => self.margin[1] = absolute_length(value)?,
                "margin-bottom" => self.margin[2] = absolute_length(value)?,
                "margin-left" => self.margin[3] = absolute_length(value)?,
                "margin" => self.margins(value)?,
                "text-indent" => self.indent = absolute_length(value)?,
                // These are valid XFA formatting properties whose effect cannot
                // be approximated by dropping them. Unknown CSS is ignored as XFA
                // requires; supported vocabulary with missing semantics is explicit.
                "tab-interval" | "tab-stop" | "tab-stops" | "xfa-tab-stops" | "font-stretch"
                | "text-valign" | "page-break-before" | "page-break-after"
                | "page-break-inside" | "orphans" | "widows" | "kerning-mode"
                | "xfa-font-vscale" | "xfa-font-hscale" => {
                    return Err(super::unsupported(
                        "rich text layout property is unavailable",
                    ));
                }
                _ => (),
            }
        }
        Ok(())
    }

    fn margins(&mut self, value: &str) -> Result<()> {
        let mut values = [0.0; 4];
        let mut count = 0;
        for part in value.split_whitespace() {
            let slot = values
                .get_mut(count)
                .ok_or(Error::syntax("too many rich text margins"))?;
            *slot = absolute_length(part)?;
            count += 1;
        }
        self.margin = match count {
            1 => [values[0]; 4],
            2 => [values[0], values[1], values[0], values[1]],
            3 => [values[0], values[1], values[2], values[1]],
            4 => values,
            _ => return Err(Error::syntax("missing rich text margins")),
        };
        Ok(())
    }

    fn font(&mut self, value: &str) -> Result<()> {
        let mut rest = value.trim();
        self.bold = false;
        self.italic = false;
        loop {
            let (token, tail) = rest
                .split_once(char::is_whitespace)
                .ok_or(Error::syntax("incomplete rich text font shorthand"))?;
            rest = tail.trim_start();
            if token == "normal" {
                continue;
            }
            if matches!(token, "italic" | "oblique") {
                self.italic = true;
                continue;
            }
            if token == "bold"
                || matches!(
                    token,
                    "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800" | "900"
                )
            {
                self.bold = weight(token)?;
                continue;
            }
            let (size, leading) = token
                .split_once('/')
                .map_or((token, None), |(a, b)| (a, Some(b)));
            self.size = absolute_length(size)?;
            if self.size <= 0.0 {
                return Err(Error::syntax("rich text font size must be positive"));
            }
            self.leading = leading
                .map(|value| absolute_length(value).and_then(nonnegative))
                .transpose()?;
            if rest.starts_with('/') {
                let (leading, family) = rest[1..]
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .ok_or(Error::syntax("incomplete rich text font shorthand"))?;
                self.leading = Some(nonnegative(absolute_length(leading)?)?);
                rest = family.trim_start();
            }
            self.family = family(rest)?;
            return Ok(());
        }
    }
}

fn absolute_length(value: &str) -> Result<f64> {
    if !["pt", "in", "cm", "mm"]
        .iter()
        .any(|unit| value.ends_with(unit))
        && value.parse::<f64>().ok() != Some(0.0)
    {
        return Err(super::unsupported(
            "relative or unitless character/paragraph measurements are unavailable",
        ));
    }
    length(value)
}

fn family(value: &str) -> Result<Name> {
    let value = value.trim();
    if value.contains(',') {
        return Err(super::unsupported(
            "rich text font fallback lists are unavailable",
        ));
    }
    let value = if value.starts_with(['\'', '"']) {
        let quote = value.as_bytes()[0];
        if value.len() < 2 || value.as_bytes().last() != Some(&quote) {
            return Err(Error::syntax("unterminated rich text font family"));
        }
        &value[1..value.len() - 1]
    } else {
        value
    };
    if value.is_empty() {
        return Err(Error::syntax("empty rich text font family"));
    }
    Ok(Name::new(value.as_bytes()))
}

fn weight(value: &str) -> Result<bool> {
    match value {
        "normal" | "100" | "200" | "300" | "400" | "500" => Ok(false),
        "bold" | "600" | "700" | "800" | "900" => Ok(true),
        _ => Err(Error::syntax("invalid rich text font weight")),
    }
}

fn italic(value: &str) -> Result<bool> {
    match value {
        "normal" => Ok(false),
        "italic" | "oblique" => Ok(true),
        _ => Err(Error::syntax("invalid rich text font style")),
    }
}

fn percentage(value: &str) -> Result<f64> {
    let number = value
        .strip_suffix('%')
        .ok_or(Error::syntax("rich text scale requires a percentage"))?;
    let number = number
        .parse::<f64>()
        .map_err(|_| Error::syntax("invalid rich text percentage"))?
        / 100.0;
    if !number.is_finite() {
        return Err(Error::limit("rich text scale exceeds numeric range"));
    }
    if number <= 0.0 {
        return Err(Error::syntax("rich text scale must be positive"));
    }
    Ok(number)
}

// XFA3.3 chapter2 measurements default to inches. Relative coefficients are
// retained only by letter-spacing; other properties require explicit units.
pub(super) fn length(value: &str) -> Result<f64> {
    let (number, factor) = [
        ("pt", 1.0),
        ("in", 72.0),
        ("cm", 72.0 / 2.54),
        ("mm", 72.0 / 25.4),
        ("em", 1.0),
        ("%", 0.01),
    ]
    .into_iter()
    .find_map(|(unit, factor)| value.strip_suffix(unit).map(|value| (value, factor)))
    .unwrap_or((value, 72.0));
    let number = number
        .trim()
        .parse::<f64>()
        .map_err(|_| Error::syntax("invalid rich text measurement"))?
        * factor;
    if !number.is_finite() {
        return Err(Error::limit("rich text measurement exceeds numeric range"));
    }
    Ok(number)
}

fn nonnegative(value: f64) -> Result<f64> {
    if value < 0.0 {
        return Err(Error::syntax("negative rich text line height"));
    }
    Ok(value)
}

fn decoration(value: &str) -> Result<u8> {
    let mut result = 0;
    for token in value.split_whitespace() {
        result |= match token {
            "none" => 0,
            "underline" => 1,
            "line-through" => 2,
            "overline" => 4,
            _ => return Err(super::unsupported("rich text decoration is unavailable")),
        };
    }
    Ok(result)
}

fn color(value: &str) -> Result<[f64; 3]> {
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 || !hex.is_ascii() {
            return Err(Error::syntax("invalid rich text hexadecimal color"));
        }
        let mut components = [0.0; 3];
        for (index, component) in components.iter_mut().enumerate() {
            *component = f64::from(
                u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
                    .map_err(|_| Error::syntax("invalid rich text hexadecimal color"))?,
            ) / 255.0;
        }
        return Ok(components);
    }
    let values = value
        .strip_prefix("rgb(")
        .and_then(|v| v.strip_suffix(')'))
        .ok_or(Error::syntax("invalid rich text color"))?;
    let mut values = values.split(',');
    let mut components = [0.0; 3];
    for component in &mut components {
        *component = f64::from(
            values
                .next()
                .ok_or(Error::syntax("missing rich text color component"))?
                .trim()
                .parse::<u8>()
                .map_err(|_| Error::syntax("invalid rich text color component"))?,
        ) / 255.0;
    }
    if values.next().is_some() {
        return Err(Error::syntax("excess rich text color components"));
    }
    Ok(components)
}
