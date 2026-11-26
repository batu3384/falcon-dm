//! Extension pairing: the extension shows a 6-digit code, the app shows the
//! same code and the user approves. Nothing is paired without that click.

use serde::Serialize;
use std::time::{Duration, Instant};

pub(crate) const MAX_PENDING_PAIR_REQUESTS: usize = 8;
/// An unanswered request disappears after this long.
const PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// A rejected code stays rejected, so a polling extension cannot re-prompt.
const REJECTED_TTL: Duration = Duration::from_secs(10 * 60);

pub(crate) fn is_valid_pair_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

struct Pending {
    extension_id: String,
    code: String,
    seen: Instant,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PendingPair {
    pub extension_id: String,
    pub code: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PairRequestOutcome {
    /// Waiting for the user. `is_new` = the UI should be notified.
    Pending {
        is_new: bool,
    },
    Rejected,
    TooMany,
}

#[derive(Default)]
pub struct PairingState {
    pending: Vec<Pending>,
    rejected: Vec<(String, String, Instant)>,
}

impl PairingState {
    fn purge(&mut self, now: Instant) {
        self.pending.retain(|p| now.duration_since(p.seen) < PENDING_TTL);
        self.rejected.retain(|(_, _, at)| now.duration_since(*at) < REJECTED_TTL);
    }

    pub(crate) fn request(&mut self, extension_id: &str, code: &str) -> PairRequestOutcome {
        let now = Instant::now();
        self.purge(now);
        if self.rejected.iter().any(|(id, c, _)| id == extension_id && c == code) {
            return PairRequestOutcome::Rejected;
        }
        if let Some(existing) = self.pending.iter_mut().find(|p| p.extension_id == extension_id) {
            existing.seen = now;
            // A new code from the same extension replaces the old prompt.
            let is_new = existing.code != code;
            existing.code = code.to_string();
            return PairRequestOutcome::Pending { is_new };
        }
        if self.pending.len() >= MAX_PENDING_PAIR_REQUESTS {
            return PairRequestOutcome::TooMany;
        }
        self.pending.push(Pending {
            extension_id: extension_id.to_string(),
            code: code.to_string(),
            seen: now,
        });
        PairRequestOutcome::Pending { is_new: true }
    }

    pub fn pending(&mut self) -> Vec<PendingPair> {
        self.purge(Instant::now());
        self.pending
            .iter()
            .map(|p| PendingPair { extension_id: p.extension_id.clone(), code: p.code.clone() })
            .collect()
    }

    /// Drop the prompt for an approved extension.
    pub fn resolve(&mut self, extension_id: &str) {
        self.pending.retain(|p| p.extension_id != extension_id);
    }

    pub fn reject(&mut self, extension_id: &str) {
        let now = Instant::now();
        if let Some(index) = self.pending.iter().position(|p| p.extension_id == extension_id) {
            let pending = self.pending.remove(index);
            self.rejected.push((pending.extension_id, pending.code, now));
        }
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.rejected.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const EXT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn pair_codes_are_six_digits() {
        assert!(is_valid_pair_code("482913"));
        assert!(!is_valid_pair_code("48291"));
        assert!(!is_valid_pair_code("4829131"));
        assert!(!is_valid_pair_code("48291a"));
        assert!(!is_valid_pair_code(""));
    }

    #[test]
    fn polling_the_same_code_notifies_once() {
        let mut state = PairingState::default();
        assert_eq!(state.request(EXT_A, "111111"), PairRequestOutcome::Pending { is_new: true });
        assert_eq!(state.request(EXT_A, "111111"), PairRequestOutcome::Pending { is_new: false });
        assert_eq!(
            state.pending(),
            vec![PendingPair { extension_id: EXT_A.into(), code: "111111".into() }]
        );
        // A fresh code replaces the prompt instead of stacking a second one.
        assert_eq!(state.request(EXT_A, "222222"), PairRequestOutcome::Pending { is_new: true });
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].code, "222222");
    }

    #[test]
    fn rejected_code_cannot_prompt_again_but_a_new_code_can() {
        let mut state = PairingState::default();
        state.request(EXT_A, "111111");
        state.reject(EXT_A);
        assert!(state.pending().is_empty());
        assert_eq!(state.request(EXT_A, "111111"), PairRequestOutcome::Rejected);
        assert_eq!(state.request(EXT_A, "333333"), PairRequestOutcome::Pending { is_new: true });
    }

    #[test]
    fn resolve_removes_only_that_extension() {
        let mut state = PairingState::default();
        state.request(EXT_A, "111111");
        state.request(EXT_B, "222222");
        state.resolve(EXT_A);
        assert_eq!(state.pending().len(), 1);
        assert_eq!(state.pending()[0].extension_id, EXT_B);
    }

    #[test]
    fn pending_requests_are_bounded() {
        let mut state = PairingState::default();
        for index in 0..MAX_PENDING_PAIR_REQUESTS {
            let id: String = std::iter::repeat_n((b'a' + index as u8) as char, 32).collect();
            assert!(matches!(state.request(&id, "123456"), PairRequestOutcome::Pending { .. }));
        }
        assert_eq!(
            state.request(EXT_B.replace('b', "p").as_str(), "123456"),
            PairRequestOutcome::TooMany
        );
    }
}
