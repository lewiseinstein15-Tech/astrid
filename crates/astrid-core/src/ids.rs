//! Identifiers and timestamps. All ids are prefixed, uuid-backed strings.

use chrono::Utc;
use uuid::Uuid;

/// Generate a prefixed id, e.g. `turn_9f2c…`.
pub fn new_id(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}

/// Current UTC time, RFC 3339 with seconds precision.
pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = new_id("turn");
        let b = new_id("turn");
        assert!(a.starts_with("turn_") && b.starts_with("turn_"));
        assert_ne!(a, b);
    }

    #[test]
    fn timestamp_parses() {
        let t = now_iso();
        assert!(chrono::DateTime::parse_from_rfc3339(&t).is_ok());
    }
}
