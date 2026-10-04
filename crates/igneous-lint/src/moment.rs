//! Reading dates written with Moment.js formats, for `yaml-timestamp`.
//! Writing them is `igneous_core::datefmt`.
//!
//! Parsing is strict: the whole string must match the format. Callers then
//! check the date formats back to the same string, as obsidian-linter does.

use igneous_core::datefmt;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use jiff::{Timestamp, Zoned};

/// Moment's format when given none.
pub const DEFAULT_FORMAT: &str = "YYYY-MM-DDTHH:mm:ssZ";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Tokens, longest first, as Moment's tokenizer prefers them.
const TOKENS: &[&str] = &[
    "YYYY", "MMMM", "dddd", "SSS", "MMM", "ddd", "YY", "Mo", "MM", "Do", "DD", "dd", "HH", "hh",
    "kk", "mm", "ss", "SS", "ZZ", "M", "D", "d", "E", "H", "h", "k", "m", "s", "S", "A", "a", "Z",
    "X", "x",
];

#[derive(Debug, PartialEq)]
enum Piece<'f> {
    Literal(&'f str),
    Token(&'static str),
}

/// Splits a format into tokens and literal text. `[text]` is literal, as in
/// Moment, where the brackets can't contain another `[`.
fn tokenize(format: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = format;
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[')
            && let Some(end) = inner.find(']')
            && !inner[..end].contains('[')
        {
            pieces.push(Piece::Literal(&inner[..end]));
            rest = &inner[end + 1..];
            continue;
        }
        if let Some(token) = TOKENS.iter().find(|t| rest.starts_with(**t)) {
            pieces.push(Piece::Token(token));
            rest = &rest[token.len()..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        pieces.push(Piece::Literal(&rest[..ch.len_utf8()]));
        rest = &rest[ch.len_utf8()..];
    }
    pieces
}

#[derive(Default)]
struct Fields {
    year: Option<i16>,
    month: Option<i8>,
    day: Option<i8>,
    hour: Option<i8>,
    pm: Option<bool>,
    minute: i8,
    second: i8,
    millis: i32,
    offset: Option<i32>,
    unix: Option<Timestamp>,
}

/// Reads `input` written in `format`. Times without an offset are in `tz`.
pub fn parse(input: &str, format: &str, tz: &TimeZone) -> Option<Zoned> {
    let format = if format.is_empty() {
        DEFAULT_FORMAT
    } else {
        format
    };
    let mut f = Fields::default();
    let mut rest = input;
    for piece in tokenize(format) {
        match piece {
            Piece::Literal(text) => rest = rest.strip_prefix(text)?,
            Piece::Token(token) => rest = read_token(token, rest, &mut f)?,
        }
    }
    if !rest.is_empty() {
        return None;
    }
    if let Some(ts) = f.unix {
        return Some(ts.to_zoned(tz.clone()));
    }
    let mut hour = f.hour.unwrap_or(0);
    if let Some(pm) = f.pm {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = match (pm, hour) {
            (false, 12) => 0,
            (true, 12) => 12,
            (true, h) => h + 12,
            (false, h) => h,
        };
    }
    let date = Date::new(f.year?, f.month.unwrap_or(1), f.day.unwrap_or(1)).ok()?;
    let time = Time::new(hour, f.minute, f.second, f.millis * 1_000_000).ok()?;
    let datetime = DateTime::from_parts(date, time);
    match f.offset {
        Some(seconds) => {
            let offset = Offset::from_seconds(seconds).ok()?;
            let ts = offset.to_timestamp(datetime).ok()?;
            Some(ts.to_zoned(tz.clone()))
        }
        None => datetime.to_zoned(tz.clone()).ok(),
    }
}

fn digits(rest: &str, min: usize, max: usize) -> Option<(i64, &str)> {
    let count = rest
        .bytes()
        .take(max)
        .take_while(u8::is_ascii_digit)
        .count();
    if count < min {
        return None;
    }
    Some((rest[..count].parse().ok()?, &rest[count..]))
}

fn name<'a>(rest: &'a str, names: &[&str], short: Option<usize>) -> Option<(usize, &'a str)> {
    for (i, full) in names.iter().enumerate() {
        let candidate = match short {
            Some(n) => &full[..n],
            None => full,
        };
        if rest.len() >= candidate.len()
            && rest.is_char_boundary(candidate.len())
            && rest[..candidate.len()].eq_ignore_ascii_case(candidate)
        {
            return Some((i, &rest[candidate.len()..]));
        }
    }
    None
}

fn read_token<'a>(token: &str, rest: &'a str, f: &mut Fields) -> Option<&'a str> {
    let narrow = |v: i64| i8::try_from(v).ok();
    Some(match token {
        "YYYY" => {
            let (v, r) = digits(rest, 4, 4)?;
            f.year = Some(v as i16);
            r
        }
        "YY" => {
            let (v, r) = digits(rest, 2, 2)?;
            f.year = Some(if v > 68 { 1900 + v } else { 2000 + v } as i16);
            r
        }
        "MMMM" => {
            let (i, r) = name(rest, &MONTHS, None)?;
            f.month = Some(i as i8 + 1);
            r
        }
        "MMM" => {
            let (i, r) = name(rest, &MONTHS, Some(3))?;
            f.month = Some(i as i8 + 1);
            r
        }
        "MM" | "M" | "Mo" => {
            let (v, r) = digits(rest, if token == "MM" { 2 } else { 1 }, 2)?;
            f.month = narrow(v);
            if token == "Mo" { skip_ordinal(r)? } else { r }
        }
        "DD" | "D" | "Do" => {
            let (v, r) = digits(rest, if token == "DD" { 2 } else { 1 }, 2)?;
            f.day = narrow(v);
            if token == "Do" { skip_ordinal(r)? } else { r }
        }
        "dddd" => name(rest, &WEEKDAYS, None)?.1,
        "ddd" => name(rest, &WEEKDAYS, Some(3))?.1,
        "dd" => name(rest, &WEEKDAYS, Some(2))?.1,
        "d" | "E" => digits(rest, 1, 1)?.1,
        "HH" | "H" | "kk" | "k" => {
            let (v, r) = digits(rest, if token.len() == 2 { 2 } else { 1 }, 2)?;
            f.hour = narrow(if token.starts_with('k') && v == 24 {
                0
            } else {
                v
            });
            r
        }
        "hh" | "h" => {
            let (v, r) = digits(rest, if token == "hh" { 2 } else { 1 }, 2)?;
            f.hour = narrow(v);
            f.pm.get_or_insert(false);
            r
        }
        "mm" | "m" => {
            let (v, r) = digits(rest, if token == "mm" { 2 } else { 1 }, 2)?;
            f.minute = narrow(v)?;
            r
        }
        "ss" | "s" => {
            let (v, r) = digits(rest, if token == "ss" { 2 } else { 1 }, 2)?;
            f.second = narrow(v)?;
            r
        }
        "SSS" | "SS" | "S" => {
            let (v, r) = digits(rest, token.len(), token.len())?;
            f.millis = (v * 10i64.pow(3 - token.len() as u32)) as i32;
            r
        }
        "A" | "a" => {
            let lower = rest.get(..2)?.to_ascii_lowercase();
            f.pm = Some(match lower.as_str() {
                "am" => false,
                "pm" => true,
                _ => return None,
            });
            &rest[2..]
        }
        "Z" | "ZZ" => {
            if let Some(r) = rest.strip_prefix('Z') {
                f.offset = Some(0);
                return Some(r);
            }
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hours, r) = digits(&rest[1..], 2, 2)?;
            let r = r.strip_prefix(':').unwrap_or(r);
            let (minutes, r) = digits(r, 0, 2).unwrap_or((0, r));
            f.offset = Some(sign * (hours as i32 * 3600 + minutes as i32 * 60));
            r
        }
        "X" | "x" => {
            let negative = rest.starts_with('-');
            let body = rest.trim_start_matches('-');
            let (v, r) = digits(body, 1, 18)?;
            let v = if negative { -v } else { v };
            f.unix = Some(if token == "X" {
                Timestamp::from_second(v).ok()?
            } else {
                Timestamp::from_millisecond(v).ok()?
            });
            r
        }
        _ => return None,
    })
}

fn skip_ordinal(rest: &str) -> Option<&str> {
    ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|suffix| rest.strip_prefix(suffix))
}

/// Writes `time` in `format`.
pub fn format(time: &Zoned, format: &str) -> String {
    let format = if format.is_empty() {
        DEFAULT_FORMAT
    } else {
        format
    };
    datefmt::format(time, format)
}

/// Whether `input` is exactly how `format` writes some time.
pub fn matches(input: &str, format_str: &str, tz: &TimeZone) -> Option<Zoned> {
    let time = parse(input, format_str, tz)?;
    (format(&time, format_str) == input).then_some(time)
}

/// Formats commonly found in frontmatter, for guessing how a date was
/// written (obsidian-linter uses moment-parseformat for this).
const COMMON_FORMATS: &[&str] = &[
    "dddd, MMMM Do YYYY, h:mm:ss a",
    "dddd, MMMM Do YYYY, h:mm a",
    "dddd, MMMM Do YYYY",
    "MMMM Do YYYY, h:mm:ss a",
    "MMMM Do YYYY",
    "YYYY-MM-DDTHH:mm:ssZ",
    "YYYY-MM-DDTHH:mm:ss.SSSZ",
    "YYYY-MM-DDTHH:mm:ss",
    "YYYY-MM-DDTHH:mm",
    "YYYY-MM-DD HH:mm:ss",
    "YYYY-MM-DD HH:mm",
    "YYYY-MM-DD",
    "ddd, D MMM YYYY HH:mm:ss Z",
    "ddd, DD MMM YYYY HH:mm:ss Z",
    "D MMMM YYYY",
    "MMMM D, YYYY",
    "DD/MM/YYYY",
    "YYYY/MM/DD",
];

/// Guesses the format `input` was written in, if it isn't `preferred`.
pub fn guess_format(input: &str, preferred: &str, tz: &TimeZone) -> Option<&'static str> {
    if parse(input, preferred, tz).is_some() {
        return None;
    }
    COMMON_FORMATS
        .iter()
        .copied()
        .find(|f| parse(input, f, tz).is_some())
}

/// Reads an ISO 8601 time, as file times are given.
pub fn parse_iso(input: &str, tz: &TimeZone) -> Option<Zoned> {
    if let Ok(ts) = input.parse::<Timestamp>() {
        return Some(ts.to_zoned(tz.clone()));
    }
    for format in [
        "YYYY-MM-DDTHH:mm:ssZ",
        "YYYY-MM-DDTHH:mm:ss.SSSZ",
        "YYYY-MM-DDTHH:mm:ss",
        "YYYY-MM-DD",
    ] {
        if let Some(time) = parse(input, format, tz) {
            return Some(time);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let tz = TimeZone::UTC;
        let long = "dddd, MMMM Do YYYY, h:mm:ss a";
        let input = "Thursday, January 2nd 2020, 12:00:05 am";
        let time = matches(input, long, &tz).expect("parses");
        assert_eq!(time.hour(), 0);
        assert_eq!(time.second(), 5);
        assert!(matches("Thursday, January 2nd 2020, 12:00 am", long, &tz).is_none());
        let time = parse("2020-01-01T21:00:05-05:00", DEFAULT_FORMAT, &tz).unwrap();
        assert_eq!(format(&time, DEFAULT_FORMAT), "2020-01-02T02:00:05+00:00");
        assert_eq!(
            tokenize("[[[]YYYY[]]"),
            [
                Piece::Literal("["),
                Piece::Literal("["),
                Piece::Literal(""),
                Piece::Token("YYYY"),
                Piece::Literal(""),
                Piece::Literal("]"),
            ]
        );
        assert!(parse_iso("2020-01-02T00:00:00-00", &tz).is_some());
        assert_eq!(
            guess_format("Wed, 1 Jan 2020 09:00:00 -05:00", DEFAULT_FORMAT, &tz),
            Some("ddd, D MMM YYYY HH:mm:ss Z")
        );
    }
}
