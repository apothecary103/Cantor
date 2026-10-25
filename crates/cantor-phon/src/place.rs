//! Phoneme placement (docs/spec/40-algorithms.md §1–2).
//!
//! The vowel nucleus lands exactly on the note onset. A *group* spans from
//! one note onset to the next; it holds the note's nucleus (and any phonemes
//! after it) followed by the next note's leading consonants, which therefore
//! borrow time from the end of the earlier note.

/// Shortest phoneme in seconds.
pub const MIN_PHONE_SECS: f64 = 0.010;

/// Leading consonants may take at most this fraction of the group.
pub const MAX_BORROW: f64 = 0.5;

/// One phoneme in a group, before placement.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupPhone {
    /// Predicted length in seconds (from the duration model).
    pub predicted: f64,
    /// A user-pinned length in seconds, kept as is.
    pub pinned: Option<f64>,
    /// True for the next note's leading consonants at the end of the group.
    pub leading: bool,
}

/// The outcome of fitting a group.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    /// Lengths in seconds, summing to the group span.
    pub lengths: Vec<f64>,
    /// Set when the constraints could not all be met; lengths were then
    /// scaled proportionally.
    pub collision: Option<String>,
}

/// Fits a group's phonemes to `span` seconds.
pub fn fit_group(phones: &[GroupPhone], span: f64) -> Fitted {
    let n = phones.len();
    if n == 0 {
        return Fitted {
            lengths: vec![],
            collision: None,
        };
    }
    let pred = |p: &GroupPhone| p.predicted.max(MIN_PHONE_SECS);
    let pinned: f64 = phones.iter().filter_map(|p| p.pinned).sum();
    let free_span = span - pinned;
    let lead: Vec<usize> = (0..n)
        .filter(|&i| phones[i].leading && phones[i].pinned.is_none())
        .collect();
    let rest: Vec<usize> = (0..n)
        .filter(|&i| !phones[i].leading && phones[i].pinned.is_none())
        .collect();

    let need_min = (lead.len() + rest.len()) as f64 * MIN_PHONE_SECS;
    if free_span < need_min || (rest.is_empty() && lead.is_empty() && free_span.abs() > 1e-9) {
        return proportional(phones, span, "phonemes do not fit their notes");
    }

    // Leading consonants: predicted, capped at MAX_BORROW of the span,
    // never below the minimum, and leaving the minimum for the others.
    let lead_pred: f64 = lead.iter().map(|&i| pred(&phones[i])).sum();
    let cap = (MAX_BORROW * span)
        .min(free_span - rest.len() as f64 * MIN_PHONE_SECS)
        .max(lead.len() as f64 * MIN_PHONE_SECS);
    let lead_total = lead_pred.min(cap);
    let mut lengths = vec![0.0; n];
    for (i, p) in phones.iter().enumerate() {
        if let Some(l) = p.pinned {
            lengths[i] = l;
        }
    }
    distribute(&mut lengths, phones, &lead, lead_total);
    distribute(&mut lengths, phones, &rest, free_span - lead_total);
    if rest.is_empty() && (free_span - lead_total).abs() > 1e-9 {
        // No nucleus to absorb the remainder: stretch the consonants.
        distribute(&mut lengths, phones, &lead, free_span);
    }
    Fitted {
        lengths,
        collision: None,
    }
}

/// Splits `total` over `idx` in proportion to prediction, each at least
/// the minimum.
fn distribute(lengths: &mut [f64], phones: &[GroupPhone], idx: &[usize], total: f64) {
    if idx.is_empty() {
        return;
    }
    let floor = MIN_PHONE_SECS;
    let extra = total - floor * idx.len() as f64;
    let weights: Vec<f64> = idx
        .iter()
        .map(|&i| (phones[i].predicted - floor).max(0.0) + 1e-9)
        .collect();
    let wsum: f64 = weights.iter().sum();
    for (k, &i) in idx.iter().enumerate() {
        lengths[i] = floor + extra * weights[k] / wsum;
    }
}

fn proportional(phones: &[GroupPhone], span: f64, why: &str) -> Fitted {
    let w: Vec<f64> = phones
        .iter()
        .map(|p| p.pinned.unwrap_or(p.predicted).max(1e-6))
        .collect();
    let sum: f64 = w.iter().sum();
    Fitted {
        lengths: w.iter().map(|x| span * x / sum).collect(),
        collision: Some(format!(
            "{why}: {} phonemes need more than {:.0} ms; scaled proportionally",
            phones.len(),
            span * 1000.0
        )),
    }
}

/// Converts consecutive lengths in seconds to whole frames with cumulative
/// rounding, so frame counts sum to the rounded total.
pub fn to_frames(lengths: &[f64], frame_secs: f64) -> Vec<i64> {
    let mut out = Vec::with_capacity(lengths.len());
    let (mut acc, mut prev) = (0.0, 0i64);
    for l in lengths {
        acc += l;
        let f = (acc / frame_secs).round() as i64;
        out.push(f - prev);
        prev = f;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ph(predicted: f64, leading: bool) -> GroupPhone {
        GroupPhone {
            predicted,
            pinned: None,
            leading,
        }
    }

    #[test]
    fn fills_span_and_keeps_consonant_prediction() {
        let f = fit_group(&[ph(0.3, false), ph(0.08, true)], 0.5);
        assert!(f.collision.is_none());
        assert!((f.lengths.iter().sum::<f64>() - 0.5).abs() < 1e-9);
        assert!((f.lengths[1] - 0.08).abs() < 1e-9);
    }

    #[test]
    fn caps_borrowing_at_half() {
        let f = fit_group(&[ph(0.1, false), ph(0.3, true), ph(0.3, true)], 0.4);
        let lead: f64 = f.lengths[1] + f.lengths[2];
        assert!((lead - 0.2).abs() < 1e-9, "{lead}");
        assert!((f.lengths[0] - 0.2).abs() < 1e-9);
    }

    #[test]
    fn pinned_phonemes_keep_length() {
        let mut p = ph(0.05, true);
        p.pinned = Some(0.12);
        let f = fit_group(&[ph(0.3, false), p], 0.6);
        assert_eq!(f.lengths[1], 0.12);
        assert!((f.lengths[0] - 0.48).abs() < 1e-9);
    }

    #[test]
    fn reports_collisions() {
        let phones: Vec<_> = (0..10).map(|_| ph(0.05, false)).collect();
        let f = fit_group(&phones, 0.05);
        assert!(f.collision.is_some());
        assert!((f.lengths.iter().sum::<f64>() - 0.05).abs() < 1e-9);
    }

    #[test]
    fn frames_round_cumulatively() {
        let fr = to_frames(&[0.015, 0.015, 0.015], 0.01);
        assert_eq!(fr.iter().sum::<i64>(), 5); // round(4.5) = 5 total, no drift
    }
}
