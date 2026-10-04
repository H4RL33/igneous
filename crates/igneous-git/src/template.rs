//! Commit message templates, as in obsidian-git.
//!
//! | Placeholder | Becomes |
//! |---|---|
//! | `{{date}}` | the time of the commit, in the vault's date format |
//! | `{{hostname}}` | this computer's name |
//! | `{{numFiles}}` | how many files the commit contains |
//! | `{{files}}` | the files, grouped by change: `M a.md b.md, A c.md` |

use jiff::Zoned;

use crate::status::Entry;

/// Above this many files, `{{files}}` gives up listing them, as obsidian-git
/// does.
const MAX_LISTED: usize = 100;

pub struct Context<'a> {
    pub now: Zoned,
    pub date_format: &'a str,
    pub hostname: &'a str,
    /// The staged entries the commit will contain.
    pub files: &'a [Entry],
}

pub fn render(template: &str, ctx: &Context) -> String {
    let mut out = template.to_owned();
    if out.contains("{{numFiles}}") {
        out = out.replace("{{numFiles}}", &ctx.files.len().to_string());
    }
    if out.contains("{{hostname}}") {
        out = out.replace("{{hostname}}", ctx.hostname);
    }
    if out.contains("{{files}}") {
        out = out.replace("{{files}}", &files(ctx.files));
    }
    if out.contains("{{date}}") {
        let date = igneous_core::datefmt::format(&ctx.now, ctx.date_format);
        out = out.replace("{{date}}", &date);
    }
    out
}

fn files(entries: &[Entry]) -> String {
    if entries.len() >= MAX_LISTED {
        return "Too many files to list".to_owned();
    }
    let mut groups: Vec<(char, Vec<&str>)> = Vec::new();
    for entry in entries {
        let letter = if entry.index == '.' {
            entry.letter()
        } else {
            entry.index
        };
        match groups.iter_mut().find(|(l, _)| *l == letter) {
            Some((_, paths)) => paths.push(&entry.path),
            None => groups.push((letter, vec![&entry.path])),
        }
    }
    groups
        .iter()
        .map(|(letter, paths)| format!("{letter} {}", paths.join(" ")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// This computer's name, for `{{hostname}}`.
pub fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, index: char) -> Entry {
        Entry {
            path: path.to_owned(),
            orig_path: None,
            index,
            worktree: '.',
            conflicted: false,
        }
    }

    fn ctx(files: &[Entry]) -> Context<'_> {
        Context {
            now: "2026-10-04T09:05:03[Europe/London]".parse().unwrap(),
            date_format: "YYYY-MM-DD HH:mm:ss",
            hostname: "kiln",
            files,
        }
    }

    #[test]
    fn placeholders() {
        let files = [entry("a.md", 'M'), entry("b.md", 'A'), entry("c.md", 'M')];
        assert_eq!(
            render("vault backup: {{date}}", &ctx(&files)),
            "vault backup: 2026-10-04 09:05:03"
        );
        assert_eq!(
            render(
                "{{numFiles}} files from {{hostname}}: {{files}}",
                &ctx(&files)
            ),
            "3 files from kiln: M a.md c.md, A b.md"
        );
        assert_eq!(
            render("{{date}} and {{date}}", &ctx(&files)),
            "2026-10-04 09:05:03 and 2026-10-04 09:05:03"
        );
        assert_eq!(render("plain", &ctx(&[])), "plain");
    }

    #[test]
    fn long_file_lists_are_summarised() {
        let files: Vec<Entry> = (0..100).map(|i| entry(&format!("{i}.md"), 'M')).collect();
        assert_eq!(render("{{files}}", &ctx(&files)), "Too many files to list");
    }
}
