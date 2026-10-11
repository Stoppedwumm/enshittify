//! Presence bookkeeping: outbound coalescing and inbound expiry.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::protocol::{PRESENCE_MAX_HZ, PRESENCE_TTL_SECS, Presence};

/// Coalesces local presence changes and releases at most one message per
/// `1 / PRESENCE_MAX_HZ` seconds. Unchanged state is never resent except
/// as a keep-alive.
#[derive(Debug)]
pub struct PresenceThrottle {
    pending: Option<Presence>,
    last_sent: Option<(Instant, Presence)>,
    interval: Duration,
    keepalive: Duration,
}

impl Default for PresenceThrottle {
    fn default() -> Self {
        PresenceThrottle {
            pending: None,
            last_sent: None,
            interval: Duration::from_secs(1) / PRESENCE_MAX_HZ,
            keepalive: Duration::from_secs(PRESENCE_TTL_SECS / 3),
        }
    }
}

impl PresenceThrottle {
    pub fn update(&mut self, p: Presence) {
        self.pending = Some(p);
    }

    /// Returns a message to send now, if any.
    pub fn poll(&mut self, now: Instant) -> Option<Presence> {
        let due = match &self.last_sent {
            None => true,
            Some((t, _)) => now.duration_since(*t) >= self.interval,
        };
        if !due {
            return None;
        }
        if let Some(p) = self.pending.take() {
            if self.last_sent.as_ref().is_some_and(|(_, last)| *last == p)
                && self
                    .last_sent
                    .as_ref()
                    .is_some_and(|(t, _)| now.duration_since(*t) < self.keepalive)
            {
                return None;
            }
            self.last_sent = Some((now, p.clone()));
            return Some(p);
        }
        // Keep-alive so peers don't expire us while idle.
        if let Some((t, p)) = &self.last_sent
            && now.duration_since(*t) >= self.keepalive
        {
            let p = p.clone();
            self.last_sent = Some((now, p.clone()));
            return Some(p);
        }
        None
    }
}

/// Remote collaborators' latest presence, expiring after 15 s of silence.
#[derive(Debug, Default)]
pub struct PresenceBoard {
    peers: HashMap<u64, (Instant, Presence)>,
}

impl PresenceBoard {
    pub fn apply(&mut self, p: Presence, now: Instant) {
        self.peers.insert(p.peer_id, (now, p));
    }

    pub fn remove(&mut self, peer: u64) {
        self.peers.remove(&peer);
    }

    pub fn expire(&mut self, now: Instant) {
        let ttl = Duration::from_secs(PRESENCE_TTL_SECS);
        self.peers.retain(|_, (t, _)| now.duration_since(*t) < ttl);
    }

    pub fn peers(&self) -> impl Iterator<Item = &Presence> {
        self.peers.values().map(|(_, p)| p)
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_caps_rate_and_coalesces() {
        let mut t = PresenceThrottle::default();
        let t0 = Instant::now();
        let p = |tick| Presence {
            playhead_tick: tick,
            ..Default::default()
        };
        t.update(p(1));
        assert_eq!(t.poll(t0).unwrap().playhead_tick, 1);
        // 100 updates within 10 ms → nothing until the interval passes.
        for i in 0..100 {
            t.update(p(i));
            assert!(t.poll(t0 + Duration::from_millis(i as u64 / 10)).is_none());
        }
        let sent = t.poll(t0 + Duration::from_millis(60)).unwrap();
        assert_eq!(sent.playhead_tick, 99, "latest state wins");
        // Over one second of continuous changes: ≤ 20 messages, ≥ 10.
        let mut n = 0;
        for ms in 0..1000u64 {
            t.update(p(ms as i64 + 1000));
            if t.poll(t0 + Duration::from_millis(100 + ms)).is_some() {
                n += 1;
            }
        }
        assert!((10..=21).contains(&n), "{n} msgs/s");
    }

    #[test]
    fn board_expires() {
        let mut b = PresenceBoard::default();
        let t0 = Instant::now();
        b.apply(
            Presence {
                peer_id: 1,
                ..Default::default()
            },
            t0,
        );
        b.expire(t0 + Duration::from_secs(14));
        assert_eq!(b.len(), 1);
        b.expire(t0 + Duration::from_secs(16));
        assert!(b.is_empty());
    }
}
