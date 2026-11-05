//! The time a stream line states, read from the start of its text (ADR-0030, Ordering).
//!
//! Pure: [`stated_time`] is told the line, the source's zone, the arrival and this Mac's zone,
//! and answers Unix milliseconds or nothing.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use jiff::{Timestamp, Zoned};

const HALF_DAY_MS: i64 = 12 * 60 * 60 * 1000;
/// Epoch numbers at or above this are milliseconds: as seconds it is the year 5138, as
/// milliseconds 1973.
const EPOCH_MS_FROM: f64 = 1e11;
const JSON_FIELDS: [&str; 5] = ["ts", "time", "timestamp", "@timestamp", "t"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The Unix milliseconds `text` states at its start, or `None` when it states none. A time with
/// no zone is read in `source_tz` (a zone name), else in `local`.
pub(crate) fn stated_time(
    text: &str,
    source_tz: Option<&str>,
    arrived: u64,
    local: &TimeZone,
) -> Option<u64> {
    let zone = source_tz
        .and_then(|name| TimeZone::get(name).ok())
        .unwrap_or_else(|| local.clone());
    let text = text.trim_start();
    iso(text, &zone)
        .or_else(|| time_of_day(text, &zone, arrived))
        .or_else(|| syslog(text, &zone, arrived))
        .or_else(|| json(text, &zone))
}

/// A cursor over the bytes of a line.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cur<'_> {
    fn eat(&mut self, c: u8) -> bool {
        let hit = self.b.get(self.i) == Some(&c);
        self.i += hit as usize;
        hit
    }

    /// Exactly `n` digits.
    fn num(&mut self, n: usize) -> Option<i32> {
        let digits = self.b.get(self.i..self.i + n)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.i += n;
        Some(digits.iter().fold(0, |v, d| v * 10 + (d - b'0') as i32))
    }

    /// `.fff` (or `,fff`), up to nanoseconds.
    fn fraction(&mut self) -> Option<i32> {
        if !(self.eat(b'.') || self.eat(b',')) {
            return Some(0);
        }
        let start = self.i;
        while self.b.get(self.i).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
        }
        let digits = &self.b[start..self.i];
        if digits.is_empty() {
            return None;
        }
        let mut nanos = 0i32;
        for k in 0..9 {
            nanos = nanos * 10 + digits.get(k).map_or(0, |d| (d - b'0') as i32);
        }
        Some(nanos)
    }

    fn at_digit(&self) -> bool {
        self.b.get(self.i).is_some_and(u8::is_ascii_digit)
    }

    /// `HH:MM:SS[.fff]`, not followed by a digit.
    fn time(&mut self) -> Option<Time> {
        let (h, m, s) = {
            let h = self.num(2)?;
            self.eat(b':').then_some(())?;
            let m = self.num(2)?;
            self.eat(b':').then_some(())?;
            (h, m, self.num(2)?)
        };
        let nanos = self.fraction()?;
        if self.at_digit() {
            return None;
        }
        Time::new(h as i8, m as i8, s as i8, nanos).ok()
    }

    /// `Z`, `±HH:MM`, `±HHMM` or `±HH`.
    fn offset(&mut self) -> Option<Option<Offset>> {
        if self.eat(b'Z') || self.eat(b'z') {
            return Some(Some(Offset::UTC));
        }
        let sign = match self.b.get(self.i) {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => return Some(None),
        };
        let save = self.i;
        self.i += 1;
        let parsed = (|| {
            let h = self.num(2)?;
            let had_colon = self.eat(b':');
            let m = if self.at_digit() {
                self.num(2)?
            } else if had_colon {
                return None;
            } else {
                0
            };
            Offset::from_seconds(sign * (h * 3600 + m * 60)).ok()
        })();
        match parsed {
            Some(o) if !self.at_digit() => Some(Some(o)),
            _ => {
                // Not an offset (`-` as a separator): the time stands without one.
                self.i = save;
                Some(None)
            }
        }
    }
}

fn ms(ts: Timestamp) -> Option<u64> {
    u64::try_from(ts.as_millisecond()).ok()
}

fn in_zone(dt: DateTime, zone: &TimeZone) -> Option<u64> {
    ms(zone.to_zoned(dt).ok()?.timestamp())
}

/// RFC 3339 / ISO-8601: `YYYY-MM-DD`, `T` or a space, `HH:MM:SS[.fff]`, then optionally a zone.
fn iso(text: &str, zone: &TimeZone) -> Option<u64> {
    let mut c = Cur {
        b: text.as_bytes(),
        i: 0,
    };
    let (y, mo, d) = {
        let y = c.num(4)?;
        c.eat(b'-').then_some(())?;
        let mo = c.num(2)?;
        c.eat(b'-').then_some(())?;
        (y, mo, c.num(2)?)
    };
    (c.eat(b'T') || c.eat(b't') || c.eat(b' ')).then_some(())?;
    let time = c.time()?;
    let date = Date::new(y as i16, mo as i8, d as i8).ok()?;
    let dt = date.to_datetime(time);
    match c.offset()? {
        Some(offset) => ms(offset.to_timestamp(dt).ok()?),
        None => in_zone(dt, zone),
    }
}

/// `HH:MM:SS[.fff]` alone: today's date in `zone`, or yesterday's when today's puts the time
/// more than 12 hours after `arrived`.
fn time_of_day(text: &str, zone: &TimeZone, arrived: u64) -> Option<u64> {
    let time = Cur {
        b: text.as_bytes(),
        i: 0,
    }
    .time()?;
    let now = Timestamp::from_millisecond(arrived as i64)
        .ok()?
        .to_zoned(zone.clone());
    let today = in_zone(now.date().to_datetime(time), zone)? as i64;
    if today - arrived as i64 > HALF_DAY_MS {
        in_zone(now.date().yesterday().ok()?.to_datetime(time), zone)
    } else {
        Some(today as u64)
    }
}

/// Syslog, `Oct 29 15:04:05` (the day may be space-padded): the year is the arrival's, or the
/// year before when that puts the line more than 12 hours after `arrived`.
fn syslog(text: &str, zone: &TimeZone, arrived: u64) -> Option<u64> {
    let mut c = Cur {
        b: text.as_bytes(),
        i: 0,
    };
    let name = text.get(..3)?;
    let month = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(name))? as i8 + 1;
    c.i = 3;
    c.eat(b' ').then_some(())?;
    c.eat(b' ');
    let day = match c.num(2) {
        Some(d) => d,
        None => c.num(1)?,
    };
    c.eat(b' ').then_some(())?;
    let time = c.time()?;
    let year = Timestamp::from_millisecond(arrived as i64)
        .ok()?
        .to_zoned(zone.clone())
        .year();
    let at = |year: i16| -> Option<u64> {
        in_zone(
            Date::new(year, month, day as i8).ok()?.to_datetime(time),
            zone,
        )
    };
    let this = at(year)?;
    if this as i64 - arrived as i64 > HALF_DAY_MS {
        at(year - 1)
    } else {
        Some(this)
    }
}

/// A line that is a JSON object with a string or epoch-number field `ts`, `time`, `timestamp`,
/// `@timestamp` or `t`; the first of those that holds a time wins.
fn json(text: &str, zone: &TimeZone) -> Option<u64> {
    if !text.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text.trim_end()).ok()?;
    let object = value.as_object()?;
    JSON_FIELDS
        .iter()
        .find_map(|name| match object.get(*name)? {
            serde_json::Value::String(s) => iso(s.trim(), zone),
            serde_json::Value::Number(n) => epoch(n.as_f64()?),
            _ => None,
        })
}

fn epoch(n: f64) -> Option<u64> {
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let millis = if n >= EPOCH_MS_FROM { n } else { n * 1000.0 };
    Some(millis.round() as u64)
}

/// This Mac's time zone.
pub(crate) fn local_zone() -> TimeZone {
    Zoned::now().time_zone().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> u64 {
        ms(s.parse::<Timestamp>().unwrap()).unwrap()
    }

    fn zone(name: &str) -> TimeZone {
        TimeZone::get(name).unwrap()
    }

    /// 2026-10-29 20:00:00 UTC.
    const ARRIVED: u64 = 1_793_304_000_000;

    fn read(text: &str, tz: Option<&str>) -> Option<u64> {
        stated_time(text, tz, ARRIVED, &zone("America/New_York"))
    }

    #[test]
    fn iso_with_a_zone_or_a_fraction() {
        let want = utc("2026-10-29T15:04:05.250Z");
        assert_eq!(read("2026-10-29T15:04:05.250Z msg", None), Some(want));
        assert_eq!(read("2026-10-29 15:04:05.25Z msg", None), Some(want));
        assert_eq!(read("2026-10-29T11:04:05.250-04:00 msg", None), Some(want));
        assert_eq!(read("2026-10-29T11:04:05.250-0400 msg", None), Some(want));
        assert_eq!(read("2026-10-29T17:04:05.250+02 msg", None), Some(want));
    }

    #[test]
    fn a_zone_less_iso_time_is_read_in_the_source_zone_else_the_macs() {
        let text = "2026-10-29 15:04:05 x";
        assert_eq!(read(text, Some("UTC")), Some(utc("2026-10-29T15:04:05Z")));
        assert_eq!(read(text, None), Some(utc("2026-10-29T19:04:05Z")));
        assert_eq!(
            read(text, Some("Asia/Tokyo")),
            Some(utc("2026-10-29T06:04:05Z"))
        );
    }

    #[test]
    fn a_time_of_day_is_today_or_yesterday() {
        assert_eq!(
            read("19:30:00.123 x", Some("UTC")),
            Some(utc("2026-10-29T19:30:00.123Z"))
        );
        // 23:00 UTC today is 3 h after arrival: today.
        assert_eq!(
            read("23:00:00 x", Some("UTC")),
            Some(utc("2026-10-29T23:00:00Z"))
        );
        // 09:00 UTC today is before arrival: today.
        assert_eq!(
            read("09:00:00 x", Some("UTC")),
            Some(utc("2026-10-29T09:00:00Z"))
        );
        // Arrived 02:00 UTC, line says 23:30: today would be 21.5 h ahead, so yesterday.
        let arrived = utc("2026-10-30T02:00:00Z");
        assert_eq!(
            stated_time("23:30:00 x", Some("UTC"), arrived, &zone("UTC")),
            Some(utc("2026-10-29T23:30:00Z"))
        );
    }

    #[test]
    fn syslog_takes_the_arrivals_year() {
        assert_eq!(
            read("Oct 29 15:04:05 host x", Some("UTC")),
            Some(utc("2026-10-29T15:04:05Z"))
        );
        for text in ["Oct  9 15:04:05 host x", "Oct 9 15:04:05 host x"] {
            assert_eq!(read(text, Some("UTC")), Some(utc("2026-10-09T15:04:05Z")));
        }
        let arrived = utc("2027-01-01T00:00:10Z");
        assert_eq!(
            stated_time("Dec 31 23:59:59 x", Some("UTC"), arrived, &zone("UTC")),
            Some(utc("2026-12-31T23:59:59Z"))
        );
    }

    #[test]
    fn a_json_line_states_its_time_in_a_known_field() {
        let want = utc("2026-10-29T15:04:05Z");
        for field in ["ts", "time", "timestamp", "@timestamp", "t"] {
            let line = format!(r#"{{"{field}":"2026-10-29T15:04:05Z","m":"x"}}"#);
            assert_eq!(read(&line, None), Some(want), "{field}");
        }
        assert_eq!(
            read(r#"{"m":"x","other":"2026-10-29T15:04:05Z"}"#, None),
            None
        );
    }

    #[test]
    fn epoch_seconds_and_milliseconds_are_told_apart_by_magnitude() {
        let want = utc("2026-10-29T15:04:05Z");
        assert_eq!(read(r#"{"ts":1793286245}"#, None), Some(want));
        assert_eq!(read(r#"{"ts":1793286245000}"#, None), Some(want));
        assert_eq!(read(r#"{"ts":1793286245.5}"#, None), Some(want + 500));
    }

    #[test]
    fn only_the_start_of_the_line_counts() {
        assert_eq!(read("error at 2026-10-29T15:04:05Z", None), None);
        assert_eq!(read("took 15:04:05 to finish", None), None);
        assert_eq!(read("", None), None);
        assert_eq!(read("2026-13-45 15:04:05 x", None), None);
        assert_eq!(read("2026-10-29 15:04:05123 x", None), None);
    }
}
