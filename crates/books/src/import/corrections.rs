//! H-09: non-destructive correction of chapter boundaries and positional reupload mapping.
//! Preview-only: no database mutation or publication.
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boundary { pub line: usize, pub title: String }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewChapter { pub title: String, pub body: String }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    NoBoundaries, InvalidBoundary, FirstBoundaryCannotBeMerged,
    DuplicateTarget, DuplicateSource, SourceOutOfRange, NoAssignments,
}

#[derive(Debug, Clone)]
pub struct ChapterPreview {
    lines: Vec<String>,
    boundaries: Vec<Boundary>,
    undo: Vec<Boundary>,
}

impl ChapterPreview {
    pub fn new(text: &str, mut boundaries: Vec<Boundary>) -> Result<Self, PreviewError> {
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        boundaries.sort_by_key(|b| b.line);
        if boundaries.is_empty() { return Err(PreviewError::NoBoundaries); }
        if boundaries.iter().any(|b| b.line >= lines.len() || b.title.trim().is_empty())
            || boundaries.windows(2).any(|w| w[0].line == w[1].line) {
            return Err(PreviewError::InvalidBoundary);
        }
        Ok(Self { lines, boundaries, undo: Vec::new() })
    }
    pub fn boundaries(&self) -> &[Boundary] { &self.boundaries }
    pub fn merge_false_boundary(&mut self, line: usize) -> Result<(), PreviewError> {
        let index = self.boundaries.iter().position(|b| b.line == line)
            .ok_or(PreviewError::InvalidBoundary)?;
        if index == 0 { return Err(PreviewError::FirstBoundaryCannotBeMerged); }
        self.undo.push(self.boundaries.remove(index));
        Ok(())
    }
    pub fn undo_merge(&mut self) -> bool {
        let Some(boundary) = self.undo.pop() else { return false; };
        self.boundaries.push(boundary);
        self.boundaries.sort_by_key(|b| b.line);
        true
    }
    pub fn chapters(&self) -> Vec<PreviewChapter> {
        self.boundaries.iter().enumerate().map(|(i, b)| {
            let end = self.boundaries.get(i + 1).map_or(self.lines.len(), |next| next.line);
            PreviewChapter { title: b.title.clone(),
                body: self.lines[b.line + 1..end].join("\n").trim().to_owned() }
        }).collect()
    }
}

/// Pair existing chapter IDs with source positions, never by title or chapter number.
pub fn positional_assignments(
    target_ids: &[u64], source_count: usize, assignments: &[Option<usize>],
) -> Result<Vec<(u64, usize)>, PreviewError> {
    if target_ids.len() != assignments.len() { return Err(PreviewError::InvalidBoundary); }
    let (mut targets, mut sources) = (HashSet::new(), HashSet::new());
    let mut pairs = Vec::new();
    for (&id, &position) in target_ids.iter().zip(assignments) {
        if id == 0 || !targets.insert(id) { return Err(PreviewError::DuplicateTarget); }
        let Some(position) = position else { continue; };
        if position >= source_count { return Err(PreviewError::SourceOutOfRange); }
        if !sources.insert(position) { return Err(PreviewError::DuplicateSource); }
        pairs.push((id, position));
    }
    if pairs.is_empty() { return Err(PreviewError::NoAssignments); }
    Ok(pairs)
}

/// Only a contiguous tail beyond the final replacement is suggested for creation.
pub fn suggested_new_tail(source_count: usize, assignments: &[Option<usize>]) -> Vec<usize> {
    let Some(last) = assignments.iter().filter_map(|&p| p).filter(|&p| p < source_count).max() else {
        return Vec::new();
    };
    (last + 1..source_count).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_is_lossless_and_undoable() {
        let mut p = ChapterPreview::new(
            "Глава 1\nТекст\nГлава Ассоциации придворных магов\nПродолжение\nГлава 2\nФинал",
            vec![Boundary { line: 0, title: "Глава 1".into() },
                 Boundary { line: 2, title: "Ложная глава".into() },
                 Boundary { line: 4, title: "Глава 2".into() }],
        ).unwrap();
        p.merge_false_boundary(2).unwrap();
        assert_eq!(p.chapters().len(), 2);
        assert!(p.chapters()[0].body.contains("Глава Ассоциации придворных магов"));
        assert!(p.undo_merge());
        assert_eq!(p.chapters().len(), 3);
    }
    #[test]
    fn first_boundary_is_protected() {
        let mut p = ChapterPreview::new("Глава 1\nТекст", vec![
            Boundary { line: 0, title: "Глава 1".into() }]).unwrap();
        assert_eq!(p.merge_false_boundary(0), Err(PreviewError::FirstBoundaryCannotBeMerged));
    }
    #[test]
    fn mapping_uses_positions_and_suggests_tail() {
        assert_eq!(positional_assignments(&[101, 102], 4, &[Some(1), Some(0)]).unwrap(),
            vec![(101, 1), (102, 0)]);
        assert_eq!(suggested_new_tail(6, &[Some(0), None, Some(2)]), vec![3, 4, 5]);
    }
    #[test]
    fn invalid_mapping_is_rejected() {
        assert_eq!(positional_assignments(&[1, 2], 2, &[Some(0), Some(0)]),
            Err(PreviewError::DuplicateSource));
        assert_eq!(positional_assignments(&[1], 2, &[Some(2)]),
            Err(PreviewError::SourceOutOfRange));
    }
}
