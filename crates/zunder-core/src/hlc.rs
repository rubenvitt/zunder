//! Hybrid Logical Clock.
//!
//! Wire format: `"<wall_ms>:<counter>:<node>"` with `wall_ms` zero-padded to
//! 13 digits, `counter` zero-padded to 5 digits and `node` matching
//! `[a-z0-9-]{1,36}`. Because of the fixed widths, comparing the strings
//! lexicographically is identical to comparing `(wall_ms, counter, node)`.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Largest representable `wall_ms` (13 decimal digits).
pub const MAX_WALL_MS: u64 = 9_999_999_999_999;
/// Largest representable counter (5 decimal digits).
pub const MAX_COUNTER: u32 = 99_999;
/// Maximum length of a node ID.
pub const MAX_NODE_LEN: usize = 36;

/// Errors when parsing HLC strings or node IDs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HlcError {
    #[error("invalid node id {0:?}: expected [a-z0-9-]{{1,36}}")]
    InvalidNode(String),
    #[error("invalid HLC {0:?}: expected <13-digit wall_ms>:<5-digit counter>:<node>")]
    InvalidFormat(String),
}

/// Validated node ID (`[a-z0-9-]{1,36}`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(String);

impl NodeId {
    /// Validates and wraps a node ID.
    pub fn new(node: impl Into<String>) -> Result<Self, HlcError> {
        let node = node.into();
        if Self::is_valid(&node) {
            Ok(Self(node))
        } else {
            Err(HlcError::InvalidNode(node))
        }
    }

    /// Returns whether `node` is a valid node ID.
    pub fn is_valid(node: &str) -> bool {
        (1..=MAX_NODE_LEN).contains(&node.len())
            && node
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for NodeId {
    type Err = HlcError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// A single HLC timestamp.
///
/// The derived ordering compares `(wall_ms, counter, node)`, which equals the
/// lexicographic order of the string form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    wall_ms: u64,
    counter: u32,
    node: NodeId,
}

impl Hlc {
    /// Builds an HLC, checking the numeric ranges of the wire format.
    pub fn new(wall_ms: u64, counter: u32, node: NodeId) -> Result<Self, HlcError> {
        if wall_ms > MAX_WALL_MS || counter > MAX_COUNTER {
            return Err(HlcError::InvalidFormat(format!(
                "{wall_ms}:{counter}:{node}"
            )));
        }
        Ok(Self {
            wall_ms,
            counter,
            node,
        })
    }

    pub fn wall_ms(&self) -> u64 {
        self.wall_ms
    }

    pub fn counter(&self) -> u32 {
        self.counter
    }

    pub fn node(&self) -> &NodeId {
        &self.node
    }

    /// Strictly parses the wire format.
    pub fn parse(s: &str) -> Result<Self, HlcError> {
        let err = || HlcError::InvalidFormat(s.to_owned());
        let bytes = s.as_bytes();
        // 13 digits + ':' + 5 digits + ':' + at least one node char
        if bytes.len() < 21 || bytes[13] != b':' || bytes[19] != b':' {
            return Err(err());
        }
        let wall = &s[..13];
        let counter = &s[14..19];
        let node = &s[20..];
        if !wall.bytes().all(|b| b.is_ascii_digit()) || !counter.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(err());
        }
        let node = NodeId::new(node).map_err(|_| err())?;
        Ok(Self {
            wall_ms: wall.parse().map_err(|_| err())?,
            counter: counter.parse().map_err(|_| err())?,
            node,
        })
    }

    /// Compares only the time part (`wall_ms`, `counter`), ignoring the node.
    pub fn cmp_time(&self, other: &Hlc) -> Ordering {
        (self.wall_ms, self.counter).cmp(&(other.wall_ms, other.counter))
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:013}:{:05}:{}", self.wall_ms, self.counter, self.node)
    }
}

impl FromStr for Hlc {
    type Err = HlcError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for Hlc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Hlc::parse(&s).map_err(de::Error::custom)
    }
}

/// Local hybrid logical clock of one node.
///
/// All methods are pure with respect to physical time: callers pass the
/// current physical time in Unix milliseconds, which keeps the clock fully
/// testable. Use [`system_time_ms`] in production code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    node: NodeId,
    wall_ms: u64,
    counter: u32,
}

impl Clock {
    /// A fresh clock that has not issued or seen any timestamp yet.
    pub fn new(node: NodeId) -> Self {
        Self {
            node,
            wall_ms: 0,
            counter: 0,
        }
    }

    /// A clock resumed from the largest HLC it has issued or seen before
    /// (e.g. persisted locally). Only the time part of `last` is used.
    pub fn resume(node: NodeId, last: &Hlc) -> Self {
        Self {
            node,
            wall_ms: last.wall_ms,
            counter: last.counter,
        }
    }

    pub fn node(&self) -> &NodeId {
        &self.node
    }

    /// The last timestamp state (issued or received) as an HLC of this node.
    pub fn last(&self) -> Hlc {
        Hlc {
            wall_ms: self.wall_ms,
            counter: self.counter,
            node: self.node.clone(),
        }
    }

    /// Issues a new timestamp that is strictly greater than every timestamp
    /// this clock has issued or received so far.
    ///
    /// `wall_ms = max(physical, last)`; if `wall_ms` did not advance, the
    /// counter is incremented. On counter overflow the clock borrows one
    /// millisecond from the future (`wall_ms + 1`, counter 0), which keeps the
    /// ordering strict without ever producing an out-of-range counter.
    pub fn now(&mut self, physical_ms: u64) -> Hlc {
        let physical_ms = physical_ms.min(MAX_WALL_MS);
        if physical_ms > self.wall_ms {
            self.wall_ms = physical_ms;
            self.counter = 0;
        } else if self.counter < MAX_COUNTER {
            self.counter += 1;
        } else if self.wall_ms < MAX_WALL_MS {
            self.wall_ms += 1;
            self.counter = 0;
        }
        // else: the clock is exhausted (year 2286); it saturates.
        self.last()
    }

    /// Raises the local clock to `max(local, remote)` after receiving a
    /// remote timestamp. The next [`Clock::now`] is then strictly greater
    /// than `remote`.
    pub fn receive(&mut self, remote: &Hlc) {
        if (remote.wall_ms, remote.counter) > (self.wall_ms, self.counter) {
            self.wall_ms = remote.wall_ms;
            self.counter = remote.counter;
        }
    }
}

/// Current Unix time in milliseconds according to the system clock.
pub fn system_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn node(s: &str) -> NodeId {
        NodeId::new(s).unwrap()
    }

    #[test]
    fn format_is_fixed_width() {
        let h = Hlc::new(1_728_650_000_000, 7, node("web-3f9a")).unwrap();
        assert_eq!(h.to_string(), "1728650000000:00007:web-3f9a");
        let small = Hlc::new(5, 0, node("a")).unwrap();
        assert_eq!(small.to_string(), "0000000000005:00000:a");
    }

    #[test]
    fn parse_roundtrip_and_rejects() {
        let s = "1728650000000:00042:server";
        let h = Hlc::parse(s).unwrap();
        assert_eq!(h.wall_ms(), 1_728_650_000_000);
        assert_eq!(h.counter(), 42);
        assert_eq!(h.node().as_str(), "server");
        assert_eq!(h.to_string(), s);
        for bad in [
            "",
            "1728650000000:00042:",
            "172865000000:00042:server",
            "1728650000000:0042:server",
            "1728650000000:00042:Server",
            "1728650000000-00042-server",
            "17286500000a0:00042:server",
            "1728650000000:00042:server:x",
            "+728650000000:00042:server",
            "1728650000000:00042:ä",
        ] {
            assert!(Hlc::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        let long_node = "a".repeat(37);
        assert!(Hlc::parse(&format!("1728650000000:00042:{long_node}")).is_err());
        assert!(Hlc::parse(&format!("1728650000000:00042:{}", "a".repeat(36))).is_ok());
    }

    #[test]
    fn node_validation() {
        assert!(NodeId::is_valid("server"));
        assert!(NodeId::is_valid("web-3f9a"));
        assert!(NodeId::is_valid("import"));
        assert!(!NodeId::is_valid(""));
        assert!(!NodeId::is_valid("Web"));
        assert!(!NodeId::is_valid("web_1"));
        assert!(!NodeId::is_valid("web:1"));
    }

    #[test]
    fn serde_as_string() {
        let h = Hlc::parse("1728650000000:00001:server").unwrap();
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"1728650000000:00001:server\"");
        assert_eq!(serde_json::from_str::<Hlc>(&json).unwrap(), h);
        assert!(serde_json::from_str::<Hlc>("\"1:2:3\"").is_err());
    }

    #[test]
    fn clock_now_and_receive() {
        let mut c = Clock::new(node("a"));
        let t1 = c.now(1000);
        assert_eq!(t1.to_string(), "0000000001000:00000:a");
        let t2 = c.now(1000);
        assert_eq!(t2.to_string(), "0000000001000:00001:a");
        // Physical clock going backwards does not move the HLC backwards.
        let t3 = c.now(900);
        assert_eq!(t3.to_string(), "0000000001000:00002:a");
        let t4 = c.now(2000);
        assert_eq!(t4.to_string(), "0000000002000:00000:a");

        let remote = Hlc::parse("0000000005000:00003:b").unwrap();
        c.receive(&remote);
        let t5 = c.now(2500);
        assert_eq!(t5.to_string(), "0000000005000:00004:a");
        assert!(t5 > remote);

        // Receiving an older timestamp changes nothing.
        c.receive(&Hlc::parse("0000000000001:00000:b").unwrap());
        assert_eq!(c.now(2500).to_string(), "0000000005000:00005:a");
    }

    #[test]
    fn clock_counter_overflow_borrows_a_millisecond() {
        let last = Hlc::new(1000, MAX_COUNTER, node("a")).unwrap();
        let mut c = Clock::resume(node("a"), &last);
        let next = c.now(1000);
        assert_eq!(next.to_string(), "0000000001001:00000:a");
        assert!(next > last);
    }

    fn arb_hlc() -> impl Strategy<Value = Hlc> {
        (0..=MAX_WALL_MS, 0..=MAX_COUNTER, "[a-z0-9-]{1,36}")
            .prop_map(|(w, c, n)| Hlc::new(w, c, NodeId::new(n).unwrap()).unwrap())
    }

    fn arb_close_hlc() -> impl Strategy<Value = Hlc> {
        // Narrow ranges to hit equal wall_ms / counters often.
        (
            0u64..3,
            0u32..3,
            prop::sample::select(vec!["a", "a-b", "b", "ab", "a0"]),
        )
            .prop_map(|(w, c, n)| Hlc::new(w, c, node(n)).unwrap())
    }

    proptest! {
        #[test]
        fn string_order_equals_tuple_order(a in arb_hlc(), b in arb_hlc()) {
            prop_assert_eq!(a.to_string().cmp(&b.to_string()), a.cmp(&b));
        }

        #[test]
        fn string_order_equals_tuple_order_close(a in arb_close_hlc(), b in arb_close_hlc()) {
            prop_assert_eq!(a.to_string().cmp(&b.to_string()), a.cmp(&b));
        }

        #[test]
        fn parse_format_roundtrip(h in arb_hlc()) {
            prop_assert_eq!(Hlc::parse(&h.to_string()).unwrap(), h);
        }

        #[test]
        fn clock_is_strictly_monotonic(
            steps in prop::collection::vec(
                prop_oneof![
                    (0u64..10_000).prop_map(Ok),
                    (0u64..10_000, 0u32..5).prop_map(Err),
                ],
                1..200,
            )
        ) {
            let mut clock = Clock::new(node("local"));
            let mut last_issued: Option<Hlc> = None;
            let mut max_seen: Option<Hlc> = None;
            for step in steps {
                match step {
                    Ok(physical) => {
                        let t = clock.now(physical);
                        if let Some(prev) = &last_issued {
                            prop_assert!(t.cmp_time(prev).is_gt(), "{} !> {}", t, prev);
                        }
                        if let Some(seen) = &max_seen {
                            prop_assert!(t.cmp_time(seen).is_gt(), "{} !> received {}", t, seen);
                        }
                        prop_assert!(t.wall_ms() >= physical);
                        last_issued = Some(t);
                    }
                    Err((wall, counter)) => {
                        let remote = Hlc::new(wall, counter, node("remote")).unwrap();
                        clock.receive(&remote);
                        if max_seen.as_ref().is_none_or(|m| remote.cmp_time(m).is_gt()) {
                            max_seen = Some(remote);
                        }
                    }
                }
            }
        }
    }
}
