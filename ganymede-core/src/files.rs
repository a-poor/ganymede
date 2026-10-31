//! Server-relative path operations shared by file management and document controllers.
use crate::jupyter::{Error, Result};

pub fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path
            .split('/')
            .any(|part| part.trim().is_empty() || part == "." || part == "..")
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return Err(Error::InvalidPath(
            "Use a relative path with a name for each folder and file".into(),
        ));
    }
    Ok(())
}
pub fn child_path(parent: &str, name: &str) -> Result<String> {
    if name.trim().is_empty() || name.contains('/') {
        return Err(Error::InvalidPath(
            "Enter a single file or folder name".into(),
        ));
    }
    let path = if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    };
    validate_path(&path)?;
    Ok(path)
}
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}
pub fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
pub fn is_within(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}
pub fn remap(path: &str, from: &str, to: &str) -> String {
    if is_within(path, from) {
        format!("{to}{}", &path[from.len()..])
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_relative_and_moves_remap_only_descendants() {
        for invalid in ["", "/tmp/x", "../x", "a/../x", "a//x", "a/", "a\\b", "a\0b"] {
            assert!(validate_path(invalid).is_err());
        }
        assert_eq!(
            child_path("project", "hello world.py").unwrap(),
            "project/hello world.py"
        );
        assert!(child_path("project", "../x").is_err());
        assert_eq!(remap("a/b/file.py", "a/b", "new"), "new/file.py");
        assert_eq!(remap("a/bb/file.py", "a/b", "new"), "a/bb/file.py");
    }
}
