//! Moment.js-style date formats (`YYYY-MM-DD HH:mm`), the syntax Obsidian users
//! already know from daily notes and templates.
//!
//! Supported tokens (English names only, as in Moment's default locale):
//!
//! | Token | Meaning |
//! |---|---|
//! | `YYYY` `YY` | year |
//! | `Q` | quarter |
//! | `MMMM` `MMM` `MM` `M` `Mo` | month |
//! | `DDDD` `DDD` | day of year |
//! | `DD` `D` `Do` | day of month |
//! | `dddd` `ddd` `dd` `d` | weekday (`d`: 0 = Sunday) |
//! | `E` | ISO weekday (1 = Monday) |
//! | `GGGG` `GG` `WW` `W` `ww` `w` | ISO week-numbering year and week |
//! | `HH` `H` `hh` `h` `kk` `k` | hour |
//! | `mm` `m` `ss` `s` | minute, second |
//! | `SSS` `SS` `S` | fractional seconds |
//! | `A` `a` | AM/PM |
//! | `ZZ` `Z` | UTC offset (`+0100`, `+01:00`) |
//! | `X` `x` | Unix seconds, milliseconds |
//! | `[text]` | literal text |

use jiff::Zoned;

const TOKENS: &[&str] = &[
    "YYYY", "GGGG", "MMMM", "DDDD", "dddd", "SSS", "MMM", "DDD", "ddd", "YY", "GG", "Mo", "MM",
    "Do", "DD", "dd", "WW", "ww", "HH", "hh", "kk", "mm", "ss", "SS", "ZZ", "Q", "M", "D", "d",
    "E", "W", "w", "H", "h", "k", "m", "s", "S", "A", "a", "Z", "X", "x",
];

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

pub fn format(time: &Zoned, pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(literal) = rest.strip_prefix('[') {
            let end = literal.find(']').unwrap_or(literal.len());
            out.push_str(&literal[..end]);
            rest = literal.get(end + 1..).unwrap_or("");
            continue;
        }
        match TOKENS.iter().find(|t| rest.starts_with(**t)) {
            Some(token) => {
                render(time, token, &mut out);
                rest = &rest[token.len()..];
            }
            None => {
                let ch = rest.chars().next().unwrap();
                out.push(ch);
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    out
}

fn render(t: &Zoned, token: &str, out: &mut String) {
    use std::fmt::Write;
    let month = t.month() as usize;
    let sunday0 = t.weekday().to_sunday_zero_offset() as usize;
    let iso = t.date().iso_week_date();
    let hour12 = match t.hour() % 12 {
        0 => 12,
        h => h,
    };
    let millis = t.subsec_nanosecond() / 1_000_000;
    let offset = t.offset().seconds();
    let (sign, offset) = if offset < 0 {
        ('-', -offset)
    } else {
        ('+', offset)
    };
    let _ = match token {
        "YYYY" => write!(out, "{:04}", t.year()),
        "YY" => write!(out, "{:02}", t.year().rem_euclid(100)),
        "GGGG" => write!(out, "{:04}", iso.year()),
        "GG" => write!(out, "{:02}", iso.year().rem_euclid(100)),
        "Q" => write!(out, "{}", (month - 1) / 3 + 1),
        "MMMM" => write!(out, "{}", MONTHS[month - 1]),
        "MMM" => write!(out, "{}", &MONTHS[month - 1][..3]),
        "MM" => write!(out, "{month:02}"),
        "M" => write!(out, "{month}"),
        "Mo" => write!(out, "{}", ordinal(month as i64)),
        "DDDD" => write!(out, "{:03}", t.day_of_year()),
        "DDD" => write!(out, "{}", t.day_of_year()),
        "DD" => write!(out, "{:02}", t.day()),
        "D" => write!(out, "{}", t.day()),
        "Do" => write!(out, "{}", ordinal(t.day() as i64)),
        "dddd" => write!(out, "{}", WEEKDAYS[sunday0]),
        "ddd" => write!(out, "{}", &WEEKDAYS[sunday0][..3]),
        "dd" => write!(out, "{}", &WEEKDAYS[sunday0][..2]),
        "d" => write!(out, "{sunday0}"),
        "E" => write!(out, "{}", t.weekday().to_monday_one_offset()),
        "WW" | "ww" => write!(out, "{:02}", iso.week()),
        "W" | "w" => write!(out, "{}", iso.week()),
        "HH" => write!(out, "{:02}", t.hour()),
        "H" => write!(out, "{}", t.hour()),
        "hh" => write!(out, "{hour12:02}"),
        "h" => write!(out, "{hour12}"),
        "kk" => write!(out, "{:02}", if t.hour() == 0 { 24 } else { t.hour() }),
        "k" => write!(out, "{}", if t.hour() == 0 { 24 } else { t.hour() }),
        "mm" => write!(out, "{:02}", t.minute()),
        "m" => write!(out, "{}", t.minute()),
        "ss" => write!(out, "{:02}", t.second()),
        "s" => write!(out, "{}", t.second()),
        "SSS" => write!(out, "{millis:03}"),
        "SS" => write!(out, "{:02}", millis / 10),
        "S" => write!(out, "{}", millis / 100),
        "A" => write!(out, "{}", if t.hour() < 12 { "AM" } else { "PM" }),
        "a" => write!(out, "{}", if t.hour() < 12 { "am" } else { "pm" }),
        "ZZ" => write!(out, "{sign}{:02}{:02}", offset / 3600, offset % 3600 / 60),
        "Z" => write!(out, "{sign}{:02}:{:02}", offset / 3600, offset % 3600 / 60),
        "X" => write!(out, "{}", t.timestamp().as_second()),
        "x" => write!(out, "{}", t.timestamp().as_millisecond()),
        _ => unreachable!("unhandled token {token}"),
    };
}

fn ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Zoned {
        s.parse().unwrap()
    }

    #[test]
    fn common_formats() {
        let t = at("2026-10-04T09:05:07.123+01:00[Europe/London]");
        assert_eq!(format(&t, "YYYY-MM-DD"), "2026-10-04");
        assert_eq!(format(&t, "YYYY-MM-DD HH:mm:ss"), "2026-10-04 09:05:07");
        assert_eq!(format(&t, "dddd, MMMM Do YYYY"), "Sunday, October 4th 2026");
        assert_eq!(format(&t, "ddd D MMM YY"), "Sun 4 Oct 26");
        assert_eq!(format(&t, "h:mm A"), "9:05 AM");
        assert_eq!(format(&t, "HH:mm:ss.SSS Z"), "09:05:07.123 +01:00");
        assert_eq!(format(&t, "GGGG-[W]WW-E"), "2026-W40-7");
        assert_eq!(format(&t, "[Today is] dddd"), "Today is Sunday");
        assert_eq!(format(&t, "DDDD Q d"), "277 4 0");
    }

    #[test]
    fn edge_hours_and_ordinals() {
        let midnight = at("2026-01-11T00:30:00+00:00[UTC]");
        assert_eq!(format(&midnight, "hh:mm a kk"), "12:30 am 24");
        assert_eq!(format(&midnight, "Do"), "11th");
        assert_eq!(
            format(&at("2026-01-22T12:00:00+00:00[UTC]"), "Do A"),
            "22nd PM"
        );
        assert_eq!(format(&at("2026-01-03T12:00:00+00:00[UTC]"), "Do"), "3rd");
    }

    #[test]
    fn unknown_text_passes_through() {
        let t = at("2026-10-04T09:05:07+00:00[UTC]");
        assert_eq!(format(&t, "YYYY/MM/DD · é"), "2026/10/04 · é");
        assert_eq!(format(&t, "[unclosed"), "unclosed");
    }
}
