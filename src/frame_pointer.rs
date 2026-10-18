//! Bounded, pure frame-pointer geometry. This does not perform DOM hit tests.

use serde_json::{Value as Json, json};

const MAX_COORDINATE: f64 = 1_000_000.0;
const MAX_CANDIDATES: usize = 9;
const MAX_PARENTS: usize = 32;
const POINT_TOLERANCE: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) left: f64,
    pub(crate) top: f64,
    pub(crate) right: f64,
    pub(crate) bottom: f64,
}

impl Rect {
    /// Zero width or height is valid and represents an empty rectangle.
    /// Reversed edges and non-finite/out-of-budget coordinates are invalid.
    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        let rect = Self {
            left: value.get("left")?.as_f64()?,
            top: value.get("top")?.as_f64()?,
            right: value.get("right")?.as_f64()?,
            bottom: value.get("bottom")?.as_f64()?,
        };
        rect.valid().then_some(rect)
    }

    pub(crate) fn to_json(self) -> Json {
        json!({"left":self.left,"top":self.top,"right":self.right,"bottom":self.bottom})
    }

    fn ordered_finite(self) -> bool {
        [self.left, self.top, self.right, self.bottom]
            .iter()
            .all(|value| value.is_finite())
            && self.left <= self.right
            && self.top <= self.bottom
    }

    fn valid(self) -> bool {
        self.ordered_finite()
            && [self.left, self.top, self.right, self.bottom]
                .iter()
                .all(|value| value.abs() <= MAX_COORDINATE)
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let rect = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
    }

    // DOM viewport right/bottom edges are excluded from hit-test coordinates.
    fn contains(self, x: f64, y: f64) -> bool {
        self.left <= x && x < self.right && self.top <= y && y < self.bottom
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mapping {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) content: Rect,
}

impl Mapping {
    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        let mapping = Self {
            x: value.get("x")?.as_f64()?,
            y: value.get("y")?.as_f64()?,
            content: Rect::from_json(value.get("content")?)?,
        };
        mapping.valid().then_some(mapping)
    }

    fn valid(self) -> bool {
        self.content.valid()
            && [self.x, self.y]
                .iter()
                .all(|scale| scale.is_finite() && *scale > 0.0 && *scale <= MAX_COORDINATE)
    }

    fn forward(self, rect: Rect) -> Result<Rect, ()> {
        let mapped = Rect {
            left: self.content.left + rect.left * self.x,
            top: self.content.top + rect.top * self.y,
            right: self.content.left + rect.right * self.x,
            bottom: self.content.top + rect.bottom * self.y,
        };
        mapped.ordered_finite().then_some(mapped).ok_or(())
    }

    fn inverse(self, rect: Rect) -> Result<Rect, ()> {
        let mapped = Rect {
            left: (rect.left - self.content.left) / self.x,
            top: (rect.top - self.content.top) / self.y,
            right: (rect.right - self.content.left) / self.x,
            bottom: (rect.bottom - self.content.top) / self.y,
        };
        mapped.ordered_finite().then_some(mapped).ok_or(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    pub(crate) mapping: Mapping,
    pub(crate) viewport: Rect,
}

impl Geometry {
    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        Some(Self {
            mapping: Mapping::from_json(value.get("mapping")?)?,
            viewport: Rect::from_json(value.get("viewport")?)?,
        })
    }

    fn valid(self) -> bool {
        self.mapping.valid() && self.viewport.valid()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) index: usize,
    pub(crate) x: f64,
    pub(crate) y: f64,
}

impl Candidate {
    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        let candidate = Self {
            index: usize::try_from(value.get("index")?.as_u64()?).ok()?,
            x: value.get("x")?.as_f64()?,
            y: value.get("y")?.as_f64()?,
        };
        candidate.valid().then_some(candidate)
    }

    pub(crate) fn to_json(self) -> Json {
        json!({"index":self.index,"x":self.x,"y":self.y})
    }

    fn valid(self) -> bool {
        self.index < MAX_CANDIDATES
            && [self.x, self.y]
                .iter()
                .all(|point| point.is_finite() && (0.0..=MAX_COORDINATE).contains(point))
    }
}

/// Clip in each parent from inner to outer, then return the remaining leaf area.
/// Empty intersections are Ok(None); invalid inputs or arithmetic are Err(()).
pub(crate) fn local_clip(
    rect: Rect,
    viewport: Rect,
    parents: &[Geometry],
) -> Result<Option<Rect>, ()> {
    if !rect.valid()
        || !viewport.valid()
        || parents.len() > MAX_PARENTS
        || parents.iter().any(|parent| !parent.valid())
    {
        return Err(());
    }
    let Some(initial) = rect.intersect(viewport) else {
        return Ok(None);
    };
    let mut clipped = initial;
    for parent in parents {
        let mapped = parent.mapping.forward(clipped)?;
        let Some(next) = mapped
            .intersect(parent.mapping.content)
            .and_then(|rect| rect.intersect(parent.viewport))
        else {
            return Ok(None);
        };
        clipped = next;
    }
    for parent in parents.iter().rev() {
        clipped = parent.mapping.inverse(clipped)?;
    }
    // Rounding during inverse transforms must not expand the original bounds.
    Ok(clipped.intersect(initial))
}

/// Accept only a bounded subset of input indices at their expected parent points.
/// `output` is the candidate array, not the surrounding DOM response envelope.
pub(crate) fn candidate_filter(
    output: &Json,
    input: &[Candidate],
    geometry: &Geometry,
) -> Option<Vec<Candidate>> {
    let output = output.as_array()?;
    if !geometry.valid() || input.len() > MAX_CANDIDATES || output.len() > MAX_CANDIDATES {
        return None;
    }
    let mut source = [None; MAX_CANDIDATES];
    for candidate in input {
        if !candidate.valid() || source[candidate.index].replace(*candidate).is_some() {
            return None;
        }
    }
    let mut seen = [false; MAX_CANDIDATES];
    let mut accepted = Vec::with_capacity(output.len());
    for value in output {
        let candidate = Candidate::from_json(value)?;
        let original = source[candidate.index]?;
        if std::mem::replace(&mut seen[candidate.index], true) {
            return None;
        }
        let x = geometry.mapping.content.left + original.x * geometry.mapping.x;
        let y = geometry.mapping.content.top + original.y * geometry.mapping.y;
        if !x.is_finite()
            || !y.is_finite()
            || (candidate.x - x).abs() > POINT_TOLERANCE
            || (candidate.y - y).abs() > POINT_TOLERANCE
            || !geometry.mapping.content.contains(x, y)
            || !geometry.viewport.contains(x, y)
            || !geometry.mapping.content.contains(candidate.x, candidate.y)
            || !geometry.viewport.contains(candidate.x, candidate.y)
        {
            return None;
        }
        accepted.push(candidate);
    }
    Some(accepted)
}
