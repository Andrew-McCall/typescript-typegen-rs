//! Type mappings for common foreign crates.
//!
//! These are plain map entries rather than trait impls, so there is no orphan
//! rule to work around and a later `map_type` call always wins.

/// A built-in set of `map_type` entries for a foreign crate. Apply with
/// [`Config::preset`](crate::config::Config::preset); a later `.map_type(..)`
/// call always overrides whatever a preset installed.
///
/// Every mapping here targets `string`, matching how each type actually
/// serialises through `serde_json` (RFC 3339 timestamps, hyphenated UUIDs,
/// decimal text, ...) — never a structural guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// `uuid::Uuid` -> `string`.
    Uuid,
    /// `chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Duration}` -> `string`.
    Chrono,
    /// `time::{OffsetDateTime, PrimitiveDateTime, Date, Time}` -> `string`.
    Time,
    /// `url::Url` -> `string`.
    Url,
    /// `rust_decimal::Decimal` and `bigdecimal::BigDecimal` -> `string`.
    Decimal,
    /// `bytes::Bytes` -> `string`.
    Bytes,
    /// Every preset above, unioned together.
    Common,
}

impl Preset {
    fn entries(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Preset::Uuid => &[("uuid::Uuid", "string")],
            Preset::Chrono => &[
                ("chrono::DateTime", "string"),
                ("chrono::NaiveDate", "string"),
                ("chrono::NaiveDateTime", "string"),
                ("chrono::NaiveTime", "string"),
                ("chrono::Duration", "string"),
            ],
            Preset::Time => &[
                ("time::OffsetDateTime", "string"),
                ("time::PrimitiveDateTime", "string"),
                ("time::Date", "string"),
                ("time::Time", "string"),
            ],
            Preset::Url => &[("url::Url", "string")],
            Preset::Decimal => {
                &[("rust_decimal::Decimal", "string"), ("bigdecimal::BigDecimal", "string")]
            }
            Preset::Bytes => &[("bytes::Bytes", "string")],
            Preset::Common => &[],
        }
    }

    /// The `(rust_path, ts)` pairs this preset installs.
    pub fn expand(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Preset::Common => [
                Preset::Uuid,
                Preset::Chrono,
                Preset::Time,
                Preset::Url,
                Preset::Decimal,
                Preset::Bytes,
            ]
            .iter()
            .flat_map(|p| p.entries().iter().copied())
            .collect(),
            other => other.entries().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn uuid_preset_maps_uuid_to_string() {
        let c = Config::new().preset(Preset::Uuid);
        assert_eq!(c.type_map().get("Uuid"), Some("string"));
    }

    #[test]
    fn chrono_preset_covers_the_common_types() {
        let c = Config::new().preset(Preset::Chrono);
        for name in ["DateTime", "NaiveDate", "NaiveDateTime", "NaiveTime", "Duration"] {
            assert!(c.type_map().get(name).is_some(), "missing mapping for {name}");
        }
    }

    #[test]
    fn common_preset_is_the_union_of_the_others() {
        let c = Config::new().preset(Preset::Common);
        assert_eq!(c.type_map().get("Uuid"), Some("string"));
        assert_eq!(c.type_map().get("Url"), Some("string"));
        assert_eq!(c.type_map().get("Decimal"), Some("string"));
    }

    #[test]
    fn a_later_map_type_overrides_a_preset() {
        let c = Config::new().preset(Preset::Uuid).map_type("uuid::Uuid", "UuidBrand");
        assert_eq!(c.type_map().get("Uuid"), Some("UuidBrand"));
    }
}
