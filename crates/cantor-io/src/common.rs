//! Helpers shared by the importers and exporters.

use crate::{ImportError, Imported};
use cantor_core::{Diagnostic, Level, PPQ, Project, Tick};

/// Diagnostics with aggregation: repeated problems become one diagnostic
/// with a count, targeted at the first occurrence.
#[derive(Debug, Default)]
pub(crate) struct Diags {
    out: Vec<(Diagnostic, usize)>,
}

impl Diags {
    /// Adds a diagnostic as is.
    pub(crate) fn push(&mut self, d: Diagnostic) {
        self.out.push((d, 1));
    }

    /// Adds an info diagnostic.
    pub(crate) fn info(&mut self, code: &str, message: impl Into<String>) {
        self.push(Diagnostic::new(Level::Info, code, message));
    }

    /// Adds a warning.
    pub(crate) fn warn(&mut self, code: &str, message: impl Into<String>) {
        self.push(Diagnostic::warning(code, message));
    }

    /// Counts a diagnostic; ones with the same code and message merge.
    pub(crate) fn count(&mut self, d: Diagnostic) {
        match self
            .out
            .iter_mut()
            .find(|(e, _)| e.code == d.code && e.message == d.message)
        {
            Some((_, n)) => *n += 1,
            None => self.out.push((d, 1)),
        }
    }

    /// Counts a warning.
    pub(crate) fn count_warn(&mut self, code: &str, message: impl Into<String>) {
        self.count(Diagnostic::warning(code, message));
    }

    /// Counts an info diagnostic.
    pub(crate) fn count_info(&mut self, code: &str, message: impl Into<String>) {
        self.count(Diagnostic::new(Level::Info, code, message));
    }

    /// Clamps `v` into `lo..=hi`, counting a warning when it was outside.
    pub(crate) fn clamp(&mut self, code: &str, what: &str, v: f64, lo: f64, hi: f64) -> f64 {
        let c = if v.is_nan() {
            lo.max(0.0).min(hi)
        } else {
            v.clamp(lo, hi)
        };
        if c != v {
            self.count_warn(code, format!("{what} outside {lo}..={hi} was clamped"));
        }
        c
    }

    /// The diagnostics, with counts appended to merged ones.
    pub(crate) fn into_vec(self) -> Vec<Diagnostic> {
        self.out
            .into_iter()
            .map(|(mut d, n)| {
                if n > 1 {
                    d.message = format!("{} ({n}×)", d.message);
                }
                d
            })
            .collect()
    }
}

/// Converts a tick count at resolution `from_ppq` to Cantor ticks: the
/// nearest tick and whether the conversion was exact.
pub(crate) fn rescale(ticks: i64, from_ppq: u32) -> (Tick, bool) {
    let num = i128::from(ticks) * i128::from(PPQ);
    let den = i128::from(from_ppq);
    let (q, r) = (num.div_euclid(den), num.rem_euclid(den));
    ((if 2 * r >= den { q + 1 } else { q }) as Tick, r == 0)
}

/// Canonicalizes and validates an imported project.
pub(crate) fn finish(mut project: Project, diags: Diags) -> Result<Imported, ImportError> {
    project.canonicalize();
    let errors: Vec<_> = project
        .validate()
        .into_iter()
        .filter(Diagnostic::is_error)
        .collect();
    if !errors.is_empty() {
        return Err(ImportError::Invalid(errors));
    }
    Ok(Imported {
        project,
        diagnostics: diags.into_vec(),
    })
}

/// Text encodings found in UTAU-world files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Enc {
    Utf8,
    ShiftJis,
    /// Neither decoded cleanly; Shift-JIS with replacement characters.
    Lossy,
}

/// Decodes text that is UTF-8 (BOM optional) or Shift-JIS.
pub(crate) fn decode(bytes: &[u8]) -> (String, Enc) {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (s.to_owned(), Enc::Utf8);
    }
    let (s, errors) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(bytes);
    (
        s.into_owned(),
        if errors { Enc::Lossy } else { Enc::ShiftJis },
    )
}

/// Formats a float without a trailing `.0` (UST/USTX style numbers).
pub(crate) fn num(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescale_exact_and_rounded() {
        assert_eq!(rescale(480, 480), (480, true));
        assert_eq!(rescale(96, 96), (480, true));
        assert_eq!(rescale(1, 1000), (0, false)); // 0.48 -> 0
        assert_eq!(rescale(-480, 240), (-960, true));
    }

    #[test]
    fn decodes_utf8_and_shift_jis() {
        assert_eq!(decode("あ".as_bytes()), ("あ".into(), Enc::Utf8));
        let (sjis, _, _) = encoding_rs::SHIFT_JIS.encode("かな");
        assert_eq!(decode(&sjis), ("かな".into(), Enc::ShiftJis));
        assert_eq!(num(120.0), "120");
        assert_eq!(num(-40.5), "-40.5");
    }
}
