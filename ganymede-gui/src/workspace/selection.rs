use ganymede_core::files;
use std::collections::HashSet;

#[derive(Clone, Default)]
pub(super) struct FileSelection {
    pub paths: HashSet<String>,
    anchor: Option<String>,
}
impl FileSelection {
    pub fn single(&mut self, path: &str) {
        self.paths.clear();
        self.anchor = (!path.is_empty()).then(|| path.to_owned());
        if !path.is_empty() {
            self.paths.insert(path.to_owned());
        }
    }
    pub fn click(&mut self, path: &str, visible: &[String], toggle: bool, range: bool) {
        self.paths.retain(|path| visible.contains(path));
        if range {
            let start = self
                .anchor
                .as_ref()
                .and_then(|anchor| visible.iter().position(|p| p == anchor));
            let end = visible.iter().position(|p| p == path);
            if let (Some(start), Some(end)) = (start, end) {
                if !toggle {
                    self.paths.clear();
                }
                self.paths
                    .extend(visible[start.min(end)..=start.max(end)].iter().cloned());
            } else if toggle {
                self.paths.insert(path.to_owned());
                self.anchor = Some(path.to_owned());
            } else {
                self.single(path);
            }
        } else if toggle {
            if !self.paths.remove(path) {
                self.paths.insert(path.to_owned());
            }
            self.anchor = Some(path.to_owned());
        } else {
            self.single(path);
        }
    }
    pub fn remap(&mut self, from: &str, to: &str) {
        self.paths = self
            .paths
            .iter()
            .map(|p| files::remap(p, from, to))
            .collect();
        self.anchor = self.anchor.as_ref().map(|p| files::remap(p, from, to));
    }
    pub fn remove(&mut self, path: &str) {
        self.paths.retain(|p| !files::is_within(p, path));
        if self
            .anchor
            .as_ref()
            .is_some_and(|p| files::is_within(p, path))
        {
            self.anchor = None;
        }
    }
}
/// A selected folder already includes its descendants in move/delete operations.
pub(super) fn selection_roots(paths: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort();
    paths.dedup();
    let mut roots: Vec<String> = Vec::new();
    for path in paths {
        if !roots.iter().any(|root| files::is_within(&path, root)) {
            roots.push(path);
        }
    }
    roots
}
#[cfg(test)]
mod tests {
    use super::*;
    fn rows() -> Vec<String> {
        ["a", "b", "c", "d", "e"].map(String::from).to_vec()
    }
    fn selected(selection: &FileSelection) -> Vec<String> {
        let mut paths: Vec<_> = selection.paths.iter().cloned().collect();
        paths.sort();
        paths
    }
    #[test]
    fn toggles_and_ranges_keep_a_stable_anchor() {
        let mut s = FileSelection::default();
        s.click("b", &rows(), false, false);
        s.click("d", &rows(), false, true);
        assert_eq!(selected(&s), ["b", "c", "d"]);
        s.click("a", &rows(), false, true);
        assert_eq!(selected(&s), ["a", "b"]);
        s.click("e", &rows(), true, false);
        assert_eq!(selected(&s), ["a", "b", "e"]);
        s.click("d", &rows(), true, true);
        assert_eq!(selected(&s), ["a", "b", "d", "e"]);
        s.click("e", &rows(), true, false);
        assert_eq!(selected(&s), ["a", "b", "d"]);
        s.click("c", &rows(), false, false);
        assert_eq!(selected(&s), ["c"]);
    }
    #[test]
    fn hidden_anchor_falls_back_and_rename_delete_keep_selection_valid() {
        let mut s = FileSelection::default();
        s.single("hidden/child");
        s.click("c", &rows(), false, true);
        assert_eq!(selected(&s), ["c"]);
        s.paths = ["folder".into(), "folder/a".into(), "folder2/a".into()].into();
        assert_eq!(selection_roots(s.paths.clone()), ["folder", "folder2/a"]);
        s.remap("folder", "moved");
        s.remove("moved");
        assert_eq!(selected(&s), ["folder2/a"]);
        s.single("");
        assert!(s.paths.is_empty());
    }
}
