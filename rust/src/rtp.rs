use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Packet {
    pub sequence: u16,
    pub timestamp: u32,
    pub payload: Vec<u8>,
    pub arrival_ms: u64,
}

pub fn parse(mut bytes: &[u8]) -> Option<Packet> {
    if bytes.len() < 12 || bytes[0] >> 6 != 2 {
        return None;
    }
    if bytes[1] & 0x7f == 0x56 {
        bytes = bytes.get(4..)?;
    }
    if bytes.len() < 12 || bytes[0] >> 6 != 2 || bytes[1] & 0x7f != 0x60 {
        return None;
    }
    let mut offset = 12 + 4 * (bytes[0] & 15) as usize;
    if bytes[0] & 0x10 != 0 {
        let ext = bytes.get(offset..offset + 4)?;
        offset += 4 + 4 * u16::from_be_bytes([ext[2], ext[3]]) as usize;
    }
    let end = if bytes[0] & 0x20 != 0 {
        let pad = *bytes.last()? as usize;
        if pad == 0 || pad > bytes.len().checked_sub(offset)? {
            return None;
        }
        bytes.len() - pad
    } else {
        bytes.len()
    };
    let payload = bytes.get(offset..end)?;
    if payload.is_empty() || payload == [0, 0x68, 0x34, 0] {
        return None;
    }
    Some(Packet {
        sequence: u16::from_be_bytes([bytes[2], bytes[3]]),
        timestamp: u32::from_be_bytes(bytes[4..8].try_into().ok()?),
        payload: payload.to_vec(),
        arrival_ms: 0,
    })
}

pub fn delta(a: u16, b: u16) -> i16 {
    a.wrapping_sub(b) as i16
}

#[derive(Default)]
pub struct Recovery {
    packets: BTreeMap<u16, Packet>,
    next: u16,
    started: bool,
    reset_fence: bool,
    flush_fence: bool,
    fence_until_ms: u64,
}
impl Recovery {
    pub const CAPACITY: usize = 256;
    pub const GAP_WAIT_MS: u64 = 60;
    pub fn reset(&mut self, next: Option<u16>, now: u64) {
        self.packets.clear();
        self.started = next.is_some();
        self.next = next.unwrap_or(0);
        self.reset_fence = next.is_some();
        self.flush_fence = next.is_some();
        self.fence_until_ms = now + 500;
    }
    pub fn enqueue(&mut self, mut packet: Packet, now: u64) -> bool {
        if !self.started {
            self.started = true;
            self.next = packet.sequence;
        }
        if now >= self.fence_until_ms {
            self.flush_fence = false;
        }
        let distance = delta(packet.sequence, self.next);
        if distance < 0 && (!self.reset_fence || self.flush_fence) {
            return false;
        }
        if distance < 0 || distance as usize >= Self::CAPACITY {
            if self.flush_fence {
                return false;
            }
            self.packets.clear();
            self.next = packet.sequence;
            self.reset_fence = false;
        }
        packet.arrival_ms = now;
        if self.packets.contains_key(&packet.sequence) {
            return false;
        }
        self.packets.insert(packet.sequence, packet);
        true
    }
    pub fn pop(&mut self, now: u64) -> Option<Packet> {
        if self.packets.is_empty() {
            return None;
        }
        if !self.packets.contains_key(&self.next) {
            let oldest = self.packets.values().map(|p| p.arrival_ms).min()?;
            if now.saturating_sub(oldest) < Self::GAP_WAIT_MS {
                return None;
            }
            self.next = *self.packets.keys().min_by_key(|s| delta(**s, self.next))?;
        }
        let out = self.packets.remove(&self.next)?;
        self.next = self.next.wrapping_add(1);
        self.reset_fence = false;
        Some(out)
    }
    pub fn missing(&self) -> Option<(u16, u16)> {
        if self.packets.contains_key(&self.next) {
            return None;
        }
        let first = *self.packets.keys().min_by_key(|s| delta(**s, self.next))?;
        let distance = delta(first, self.next);
        (distance > 0).then_some((self.next, distance as u16))
    }
    pub fn len(&self) -> usize {
        self.packets.len()
    }
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(sequence: u16) -> Packet {
        Packet {
            sequence,
            timestamp: 0,
            payload: vec![1],
            arrival_ms: 0,
        }
    }
    #[test]
    fn full_sequence_wraps_without_silence() {
        let mut r = Recovery::default();
        for i in 0..140_000u64 {
            assert!(r.enqueue(p(i as u16), i));
            assert_eq!(r.pop(i).unwrap().sequence, i as u16);
        }
    }
    #[test]
    fn losses_share_one_deadline() {
        let mut r = Recovery::default();
        r.enqueue(p(0), 0);
        r.pop(0);
        r.enqueue(p(3), 10);
        r.enqueue(p(4), 30);
        assert_eq!(r.missing(), Some((1, 2)));
        assert!(r.pop(69).is_none());
        assert_eq!(r.pop(70).unwrap().sequence, 3);
        assert_eq!(r.pop(70).unwrap().sequence, 4);
    }
    #[test]
    fn flush_fences_delayed_packets_but_recovers_mismatch() {
        let mut r = Recovery::default();
        r.reset(Some(100), 0);
        assert!(!r.enqueue(p(99), 1));
        assert!(!r.enqueue(p(2000), 20));
        assert!(r.enqueue(p(2000), 501));
        assert_eq!(r.pop(501).unwrap().sequence, 2000);
    }
    #[test]
    fn tiny_compressed_audio_is_not_a_keepalive() {
        let mut b = vec![0x80, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        b.extend_from_slice(&[1, 2, 3]);
        assert!(parse(&b).is_some());
        b.truncate(12);
        b.extend_from_slice(&[0, 0x68, 0x34, 0]);
        assert!(parse(&b).is_none());
    }
}
