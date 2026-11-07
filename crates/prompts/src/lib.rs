//! Human-editable model instructions, loaded once for a stable process lifetime.
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

/// Role-specific location under the operator's prompt directory.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// Placement-only routing instructions.
    Router,
    /// Planning, delegation and learning instructions.
    Planner,
}
impl Kind {
    const fn path(self) -> &'static str {
        match self {
            Self::Router => "router/selection.md",
            Self::Planner => "super-agent/planner.md",
        }
    }
}

/// Nonempty, bounded UTF-8 instructions. Dynamic task data is supplied separately.
#[derive(Debug, Clone)]
pub struct Prompt(String);
impl Prompt {
    /// Use the reviewed Markdown shipped with this binary.
    #[must_use]
    pub fn bundled(kind: Kind) -> Self {
        Self(
            match kind {
                Kind::Router => include_str!("../../../prompts/router/selection.md"),
                Kind::Planner => include_str!("../../../prompts/super-agent/planner.md"),
            }
            .to_owned(),
        )
    }

    /// Load an operator-owned directory. No silent fallback on invalid overrides.
    ///
    /// # Errors
    /// Rejects missing files, invalid UTF-8, empty instructions and files over 64 KiB.
    pub fn load(directory: &Path, kind: Kind) -> io::Result<Self> {
        let mut content = String::new();
        File::open(directory.join(kind.path()))?
            .take(65_537)
            .read_to_string(&mut content)?;
        if content.trim().is_empty() || content.len() > 65_536 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "prompt must contain 1–65536 bytes of nonempty UTF-8",
            ));
        }
        Ok(Self(content))
    }

    /// Exact reviewed text, without templating or interpolation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_overrides_are_bounded_and_exact() {
        let temp = tempfile::tempdir().expect("directory");
        for kind in [Kind::Router, Kind::Planner] {
            assert!(!Prompt::bundled(kind).as_str().is_empty());
            assert!(Prompt::load(temp.path(), kind).is_err());
            let path = temp.path().join(kind.path());
            std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            for content in [vec![], b" \n".to_vec(), vec![b'x'; 65_537], vec![255]] {
                std::fs::write(&path, content).expect("write");
                assert!(Prompt::load(temp.path(), kind).is_err());
            }
            for content in [
                "# Reviewed instructions\nReturn JSON.\n".to_owned(),
                "x".repeat(65_536),
            ] {
                std::fs::write(&path, &content).expect("write");
                assert_eq!(
                    Prompt::load(temp.path(), kind).expect("load").as_str(),
                    content
                );
            }
        }
    }
}
