//! History with undo/redo, per-step authorship and named checkpoints.
//!
//! Thanks to copy-on-write tiles, saving a full state costs as much as cloning a vector of
//! pointers: there is no need to record inverse operations.

use std::collections::BTreeMap;

use crate::doc::Document;

/// Who made a change: the canvas is shared between the agent and the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Author {
    Agent,
    User,
}

impl Author {
    pub fn name(self) -> &'static str {
        match self {
            Author::Agent => "agent",
            Author::User => "user",
        }
    }
}

/// One history step: who and what (a short label, e.g. `draw: circle(…)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub author: Author,
    pub label: String,
}

impl Step {
    pub fn agent(label: impl Into<String>) -> Self {
        Self {
            author: Author::Agent,
            label: label.into(),
        }
    }

    pub fn user(label: impl Into<String>) -> Self {
        Self {
            author: Author::User,
            label: label.into(),
        }
    }
}

pub struct History {
    /// `(previous state, step that turned it into the next one)`, oldest first.
    undo: Vec<(Document, Step)>,
    redo: Vec<(Document, Step)>,
    pub checkpoints: BTreeMap<String, Document>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(256)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            checkpoints: BTreeMap::new(),
            limit: limit.max(1),
        }
    }

    /// Records the state before a change and who made it.
    pub fn record(&mut self, before: Document, step: Step) {
        self.undo.push((before, step));
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Undoes one step and returns it.
    pub fn undo(&mut self, current: &mut Document) -> Option<Step> {
        let (prev, step) = self.undo.pop()?;
        self.redo
            .push((std::mem::replace(current, prev), step.clone()));
        Some(step)
    }

    /// Redoes one step and returns it.
    pub fn redo(&mut self, current: &mut Document) -> Option<Step> {
        let (next, step) = self.redo.pop()?;
        self.undo
            .push((std::mem::replace(current, next), step.clone()));
        Some(step)
    }

    /// Previous states with the step that followed each one, oldest first.
    pub fn entries(&self) -> &[(Document, Step)] {
        &self.undo
    }

    pub fn previous(&self) -> Option<&Document> {
        self.undo.last().map(|(d, _)| d)
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_keep_authors() {
        let mut h = History::default();
        let mut doc = Document::new(4, 4);
        h.record(doc.clone(), Step::agent("draw: rect"));
        doc.layers[0].raster.set(0, 0, 1);
        h.record(doc.clone(), Step::user("edit 1 px"));
        doc.layers[0].raster.set(1, 0, 1);
        assert_eq!(h.undo(&mut doc), Some(Step::user("edit 1 px")));
        assert_eq!(doc.layers[0].raster.get(1, 0), 0);
        assert_eq!(h.redo(&mut doc).map(|s| s.author), Some(Author::User));
        assert_eq!(h.entries().len(), 2);
    }
}
