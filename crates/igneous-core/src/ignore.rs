//! Which vault entries Igneous ignores.
//!
//! Every dot-file and dot-folder is ignored (this covers `.obsidian/`, `.git/`,
//! `.trash/` and `.igneous/` itself), plus any user exclusions. An exclusion
//! without glob characters matches that path and everything beneath it; one
//! with glob characters is matched against the whole vault path.

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::path::VaultPath;

#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    prefixes: Vec<VaultPath>,
    globs: GlobSet,
}

impl IgnoreRules {
    pub fn new<S: AsRef<str>>(exclusions: &[S]) -> Result<Self, globset::Error> {
        let mut prefixes = Vec::new();
        let mut globs = GlobSetBuilder::new();
        for exclusion in exclusions {
            let exclusion = exclusion.as_ref().trim();
            if exclusion.is_empty() {
                continue;
            }
            if exclusion.contains(['*', '?', '[', '{']) {
                globs.add(Glob::new(exclusion.trim_start_matches('/'))?);
            } else if let Ok(path) = VaultPath::new(exclusion) {
                prefixes.push(path);
            }
        }
        Ok(Self {
            prefixes,
            globs: globs.build()?,
        })
    }

    pub fn is_ignored(&self, path: &VaultPath) -> bool {
        is_hidden(path)
            || self.prefixes.iter().any(|prefix| path.starts_with(prefix))
            || self.globs.is_match(path.as_str())
    }
}

/// True if any component of the path starts with a dot.
pub fn is_hidden(path: &VaultPath) -> bool {
    path.components().any(|c| c.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    #[test]
    fn dot_entries_always_ignored() {
        let rules = IgnoreRules::default();
        assert!(rules.is_ignored(&p(".obsidian/workspace.json")));
        assert!(rules.is_ignored(&p(".igneous")));
        assert!(rules.is_ignored(&p("notes/.backups/old.md")));
        assert!(rules.is_ignored(&p("a/.hidden.md")));
        assert!(!rules.is_ignored(&p("notes/visible.md")));
    }

    #[test]
    fn prefix_exclusions() {
        let rules = IgnoreRules::new(&["archive", "templates/"]).unwrap();
        assert!(rules.is_ignored(&p("archive")));
        assert!(rules.is_ignored(&p("archive/2020/a.md")));
        assert!(rules.is_ignored(&p("templates/daily.md")));
        assert!(!rules.is_ignored(&p("archived/a.md")));
    }

    #[test]
    fn glob_exclusions() {
        let rules = IgnoreRules::new(&["**/*.excalidraw.md", "drafts/*"]).unwrap();
        assert!(rules.is_ignored(&p("a/b/sketch.excalidraw.md")));
        assert!(rules.is_ignored(&p("drafts/x.md")));
        assert!(!rules.is_ignored(&p("notes/x.md")));
    }
}
