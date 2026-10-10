//! Cross-platform publication-time contract.
//!
//! Every site extractor that can learn when a post was published reports it
//! through [`PublicationTime`], so consumers (CLI output, run artifacts, the
//! desktop archive) read one shape regardless of platform:
//!
//! - `at` — RFC 3339 with an explicit UTC offset (`+08:00`, `Z`), present
//!   only when the instant is known to at least hour precision. Never a
//!   synthetic anchor: a date-only or unparseable source leaves it out.
//! - `date` — the platform's own calendar date (`YYYY-MM-DD`) in `timezone`.
//!   For Xiaohongshu this is the Beijing (Asia/Shanghai) date, derived
//!   independently of the viewer's browser timezone.
//! - `timezone` — IANA name the calendar date and `at` offset belong to.
//! - `precision` — how much of the instant the source pins down.
//! - `source` — where it came from (`page_state`, `note_id`, `date_bar`,
//!   `none`, …), so a consumer can weigh it.
//! - `label` — the raw page text, kept verbatim for audit.
//!
//! Publication time, last-edited time (a second `PublicationTime` under
//! `edited`), and collection time (run metadata) are always kept separate.

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How much of a publication instant the source actually determines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    Second,
    Minute,
    Hour,
    Day,
    Unknown,
}

impl Precision {
    /// True when `at` is meaningful at this precision (hour or finer).
    pub fn has_instant(self) -> bool {
        matches!(self, Self::Second | Self::Minute | Self::Hour)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationTime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    pub timezone: String,
    pub precision: Precision,
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl PublicationTime {
    /// Validate a page-script record: an instant-level precision (`second`,
    /// `minute`, `hour`) requires an RFC 3339 `at` with an offset — a missing
    /// or unparseable `at` downgrades the record to `day`; `at` is dropped at
    /// `day`/`unknown` precision; `date` must be a real `YYYY-MM-DD` and is
    /// reconstructed from `at` when missing, and a `day` record without any
    /// date becomes `unknown`. Returns `None` when the value is not a record
    /// at all.
    pub fn from_value(value: &Value) -> Option<Self> {
        let mut record: Self = serde_json::from_value(value.clone()).ok()?;
        let parsed_at = record
            .at
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at.trim()).ok());
        if parsed_at.is_none() && record.precision.has_instant() {
            record.precision = Precision::Day;
        }
        if parsed_at.is_none() || !record.precision.has_instant() {
            record.at = None;
        }
        let date_ok = record
            .date
            .as_deref()
            .is_some_and(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok());
        if !date_ok {
            record.date = parsed_at.map(|dt| dt.date_naive().format("%Y-%m-%d").to_string());
        }
        if record.date.is_none() && record.precision == Precision::Day {
            record.precision = Precision::Unknown;
        }
        Some(record)
    }

    /// The publication instant as epoch milliseconds, when `at` is present.
    pub fn instant_ms(&self) -> Option<i64> {
        self.at
            .as_deref()
            .and_then(|at| DateTime::parse_from_rfc3339(at.trim()).ok())
            .map(|dt| dt.timestamp_millis())
    }
}
