//! Operator-authored catalog sources keep harness text in reviewable Markdown files.
use orchestrator_protocol::{
    capabilities::Capability, collaboration::CapabilityRelease, ids::ReleaseId, values::MessageText,
};
use serde::Deserialize;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    release_id: ReleaseId,
    name: MessageText,
    capability: Capability,
    supersedes: Option<ReleaseId>,
    recipe_file: PathBuf,
    instructions_file: PathBuf,
    evaluation_file: PathBuf,
}
/// Read a reviewed manifest and resolve its Markdown files relative to the manifest.
/// The returned snapshot is immutable once published; later file edits require a new release ID.
///
/// # Errors
/// Reports missing files, invalid manifests and empty or oversized harness text.
pub fn read(path: &Path) -> io::Result<CapabilityRelease> {
    let source: Source = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut directory = path.to_path_buf();
    directory.pop();
    Ok(CapabilityRelease {
        release_id: source.release_id,
        name: source.name,
        capability: source.capability,
        supersedes: source.supersedes,
        recipe: text(&directory.join(source.recipe_file))?,
        instructions: text(&directory.join(source.instructions_file))?,
        evaluation: text(&directory.join(source.evaluation_file))?,
    })
}
fn text(path: &Path) -> io::Result<MessageText> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(65_537)
        .read_to_string(&mut text)?;
    text.try_into().map_err(io::Error::other)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn sources_resolve_markdown_and_reject_missing_or_invalid_content() {
        let temp = tempfile::tempdir().expect("directory");
        let path = temp.path().join("manifest.json");
        assert!(read(&path).is_err());
        std::fs::write(&path, "bad json").expect("manifest");
        assert!(read(&path).is_err());
        let manifest = serde_json::json!({"release_id":"00000000-0000-0000-0000-000000000001","name":"Test","capability":"test.capability","supersedes":null,"recipe_file":"recipe.md","instructions_file":"instructions.md","evaluation_file":"evaluation.md"});
        std::fs::write(&path, manifest.to_string()).expect("manifest");
        for file in ["recipe.md", "instructions.md", "evaluation.md"] {
            assert!(read(&path).is_err());
            std::fs::write(temp.path().join(file), file).expect("markdown");
        }
        assert_eq!(
            read(&path).expect("snapshot").instructions.as_str(),
            "instructions.md"
        );
        for content in [vec![255], vec![], vec![b'x'; 65_537]] {
            std::fs::write(temp.path().join("recipe.md"), content).expect("markdown");
            assert!(read(&path).is_err());
        }
    }
}
