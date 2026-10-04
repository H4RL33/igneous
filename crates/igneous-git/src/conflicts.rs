//! Conflict blocks left in a file by a merge or rebase:
//!
//! ```text
//! <<<<<<< HEAD
//! upper side
//! ||||||| base          (only with merge.conflictStyle = diff3/zdiff3)
//! common ancestor
//! =======
//! lower side
//! >>>>>>> origin/main
//! ```
//!
//! In a merge the upper side is the local version; in a rebase it's the
//! upstream one, because the local commits are being replayed on top of it.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// From the start of the `<<<<<<<` line to the end of the `>>>>>>>` line,
    /// including its newline.
    pub range: Range<usize>,
    pub upper: Range<usize>,
    pub base: Option<Range<usize>>,
    pub lower: Range<usize>,
    pub upper_label: String,
    pub lower_label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Upper,
    Lower,
    /// Upper then lower.
    Both,
}

/// Every complete conflict block in `text`, in order.
pub fn find(text: &str) -> Vec<Conflict> {
    enum State {
        Outside,
        Upper,
        Base,
        Lower,
    }
    let mut found = Vec::new();
    let mut state = State::Outside;
    let (mut start, mut upper_start, mut upper_end) = (0, 0, 0);
    let (mut base, mut lower_start) = (None::<Range<usize>>, 0);
    let mut upper_label = String::new();
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches('\n').trim_end_matches('\r');
        let next = pos + line.len();
        match state {
            State::Outside => {
                if let Some(label) = marker(content, '<') {
                    start = pos;
                    upper_start = next;
                    upper_label = label.to_owned();
                    base = None;
                    state = State::Upper;
                }
            }
            State::Upper => {
                if marker(content, '|').is_some() {
                    upper_end = pos;
                    base = Some(next..next);
                    state = State::Base;
                } else if content == "=======" {
                    upper_end = pos;
                    lower_start = next;
                    state = State::Lower;
                } else if let Some(label) = marker(content, '<') {
                    // An unterminated block: start again from here.
                    start = pos;
                    upper_start = next;
                    upper_label = label.to_owned();
                }
            }
            State::Base => {
                if content == "=======" {
                    if let Some(base) = &mut base {
                        base.end = pos;
                    }
                    lower_start = next;
                    state = State::Lower;
                }
            }
            State::Lower => {
                if let Some(label) = marker(content, '>') {
                    found.push(Conflict {
                        range: start..next,
                        upper: upper_start..upper_end,
                        base: base.take(),
                        lower: lower_start..pos,
                        upper_label: std::mem::take(&mut upper_label),
                        lower_label: label.to_owned(),
                    });
                    state = State::Outside;
                }
            }
        }
        pos = next;
    }
    found
}

/// A marker line: seven `c`s, then nothing or a space and a label.
fn marker(line: &str, c: char) -> Option<&str> {
    let rest = line.strip_prefix(&c.to_string().repeat(7))?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix(' ')
    }
}

/// The text that replaces `conflict.range` to resolve it.
pub fn resolve(text: &str, conflict: &Conflict, keep: Keep) -> String {
    let upper = &text[conflict.upper.clone()];
    let lower = &text[conflict.lower.clone()];
    match keep {
        Keep::Upper => upper.to_owned(),
        Keep::Lower => lower.to_owned(),
        Keep::Both => {
            let mut both = upper.to_owned();
            if !both.is_empty() && !both.ends_with('\n') {
                both.push('\n');
            }
            both.push_str(lower);
            both
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# Home\n\
        <<<<<<< HEAD\n\
        mine\n\
        =======\n\
        theirs\n\
        more theirs\n\
        >>>>>>> origin/main\n\
        middle\n\
        <<<<<<< HEAD\n\
        a\n\
        ||||||| base\n\
        o\n\
        =======\n\
        >>>>>>> 1234abc (vault backup)\n\
        end\n";

    #[test]
    fn finds_blocks() {
        let found = find(TEXT);
        assert_eq!(found.len(), 2);
        let first = &found[0];
        assert_eq!(&TEXT[first.upper.clone()], "mine\n");
        assert_eq!(&TEXT[first.lower.clone()], "theirs\nmore theirs\n");
        assert_eq!(first.upper_label, "HEAD");
        assert_eq!(first.lower_label, "origin/main");
        assert!(TEXT[first.range.clone()].starts_with("<<<<<<<"));
        assert!(TEXT[first.range.clone()].ends_with("origin/main\n"));
        let second = &found[1];
        assert_eq!(&TEXT[second.base.clone().unwrap()], "o\n");
        assert_eq!(&TEXT[second.lower.clone()], "");
        assert_eq!(second.lower_label, "1234abc (vault backup)");
    }

    #[test]
    fn resolutions() {
        let found = find(TEXT);
        let c = &found[0];
        assert_eq!(resolve(TEXT, c, Keep::Upper), "mine\n");
        assert_eq!(resolve(TEXT, c, Keep::Lower), "theirs\nmore theirs\n");
        assert_eq!(
            resolve(TEXT, c, Keep::Both),
            "mine\ntheirs\nmore theirs\n"
        );
        let mut text = TEXT.to_owned();
        text.replace_range(c.range.clone(), &resolve(TEXT, c, Keep::Upper));
        assert!(text.starts_with("# Home\nmine\nmiddle\n"));
        assert_eq!(find(&text).len(), 1);
    }

    #[test]
    fn ignores_lookalikes_and_unfinished_blocks() {
        assert!(find("======= not a marker\n<<<<<<<< eight\n").is_empty());
        assert!(find("<<<<<<< HEAD\nmine\n=======\ntheirs\n").is_empty());
        // A stray start marker before a real block.
        let text = "<<<<<<< stray\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b\n";
        let found = find(text);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].upper.clone()], "x\n");
        // CRLF files.
        let text = "<<<<<<< HEAD\r\nx\r\n=======\r\ny\r\n>>>>>>> b\r\n";
        assert_eq!(find(text).len(), 1);
    }
}
