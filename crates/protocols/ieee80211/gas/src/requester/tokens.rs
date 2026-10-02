//! Wire-token quarantine belongs to the requesting dialog, not TX identities.
use super::*;
const TOKEN_COUNT: usize = u8::MAX as usize + 1;

pub(super) struct Tokens {
    next: u8,
    until: [Instant; TOKEN_COUNT],
}
impl Tokens {
    pub(super) const fn new() -> Self {
        Self {
            next: 0,
            until: [Instant::EPOCH; TOKEN_COUNT],
        }
    }
    pub(super) fn available(&self, now: Instant) -> Result<u8, Error> {
        for offset in 0..TOKEN_COUNT {
            let token = self.next.wrapping_add(offset as u8);
            if self.until[usize::from(token)] <= now {
                return Ok(token);
            }
        }
        Err(Error::TokensExhausted)
    }
    pub(super) fn reserve(&mut self, token: u8, until: Instant) {
        self.until[usize::from(token)] = until;
        self.next = token.wrapping_add(1);
    }
    pub(super) fn next_available_at(&self, now: Instant) -> Option<Instant> {
        self.available(now)
            .is_err()
            .then(|| *self.until.iter().min().expect("nonempty token space"))
    }
}
