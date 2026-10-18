//! Shared limits for sequential frame reads and their serialized aggregate.

use serde::Serialize;
use serde_json::{Value as Json, json};
use std::io::{self, Write};

pub(crate) const MAX_AGGREGATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_OPTIONS_PER_ELEMENT: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    /// JavaScript string length: UTF-16 code units, not Unicode scalar values.
    pub(crate) text: usize,
    pub(crate) elements: usize,
    pub(crate) nodes: usize,
    pub(crate) roots: usize,
    pub(crate) options: usize,
    /// Returned iframe metadata entries; this is not a target-document count.
    pub(crate) frames: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            text: 12_000,
            elements: 120,
            nodes: 10_000,
            roots: 64,
            options: 240,
            frames: 16,
        }
    }
}

impl Budget {
    pub(crate) fn to_json(self) -> Json {
        json!({"text":self.text,"elements":self.elements,"nodes":self.nodes,
            "roots":self.roots,"options":self.options,"frames":self.frames})
    }

    fn within(self, limit: Self) -> bool {
        self.text <= limit.text
            && self.elements <= limit.elements
            && self.nodes <= limit.nodes
            && self.roots <= limit.roots
            && self.options <= limit.options
            && self.frames <= limit.frames
    }

    fn from_json(value: &Json) -> Option<Self> {
        if value.as_object()?.len() != 6 {
            return None;
        }
        let count = |key: &str| usize::try_from(value.get(key)?.as_u64()?).ok();
        let budget = Self {
            text: count("text")?,
            elements: count("elements")?,
            nodes: count("nodes")?,
            roots: count("roots")?,
            options: count("options")?,
            frames: count("frames")?,
        };
        budget.within(Self::default()).then_some(budget)
    }

    /// Validate reported limits and usage before subtracting anything.
    /// Nodes and roots can only be range-checked from a serialized snapshot.
    pub(crate) fn consume(&mut self, page: &Json) -> Option<Self> {
        if !self.within(Self::default()) || Self::from_json(page.get("budgetLimit")?)? != *self {
            return None;
        }
        let used = Self::from_json(page.get("budgetUsed")?)?;
        if !used.within(*self) {
            return None;
        }
        let text = page.get("text")?.as_str()?;
        let elements = page.get("interactive")?.as_array()?;
        let frames = page.get("frames")?.as_array()?;
        // Early length checks also bound validation work on an invalid result.
        if elements.len() != used.elements
            || frames.len() != used.frames
            || text.encode_utf16().take(used.text + 1).count() != used.text
        {
            return None;
        }
        let mut options = 0usize;
        for element in elements {
            let element = element.as_object()?;
            if let Some(value) = element.get("options") {
                let entries = value.as_array()?;
                if entries.len() > MAX_OPTIONS_PER_ELEMENT
                    || entries.iter().any(|entry| !entry.is_object())
                {
                    return None;
                }
                options = options.checked_add(entries.len())?;
                if options > used.options {
                    return None;
                }
            }
        }
        if options != used.options || frames.iter().any(|frame| !frame.is_object()) {
            return None;
        }
        self.text -= used.text;
        self.elements -= used.elements;
        self.nodes -= used.nodes;
        self.roots -= used.roots;
        self.options -= used.options;
        self.frames -= used.frames;
        Some(used)
    }
}

struct SizeWriter {
    written: usize,
}

impl Write for SizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .written
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_AGGREGATE_BYTES)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Page read aggregate exceeds its byte limit.",
                )
            })?;
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Count JSON bytes without retaining a serialized copy. The exact 2 MiB limit
/// is inclusive. Serialize the complete response envelope for a final check.
pub(crate) fn serialized_size<T: Serialize + ?Sized>(value: &T) -> Option<usize> {
    let mut writer = SizeWriter { written: 0 };
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.written)
}
