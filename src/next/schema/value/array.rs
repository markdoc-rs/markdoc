//! An array's own properties, as V8 holds them: its elements (dense, or sparse with holes, up
//! to a length of 2^32 - 1) and any other own properties a document gave it (`.b` on an array
//! `class` is `b: true`, `.constructor` is `constructor: true`).

use super::Value;
use indexmap::IndexMap;
use std::collections::BTreeMap;

/// The largest array length, 2^32 - 1 (`ArraySetLength` rejects more).
pub const MAX_ARRAY_LENGTH: u32 = u32::MAX;

/// An array's own properties: its elements by index, its `length`, and its other own
/// properties (never an array index, never `length`), in the order they were set.
#[derive(Clone, Default)]
pub struct Array {
    elements: Elements,
    named: Option<Box<IndexMap<String, Value>>>,
}

#[derive(Clone)]
enum Elements {
    /// Every index below the length is present.
    Dense(Vec<Value>),
    /// Some index below the length is a hole: the length and the elements present.
    Sparse {
        length: u32,
        present: BTreeMap<u32, Value>,
    },
}

impl Default for Elements {
    fn default() -> Self {
        Elements::Dense(Vec::new())
    }
}

impl Array {
    /// `[...items]`: every index present.
    pub fn new(items: Vec<Value>) -> Self {
        Array {
            elements: Elements::Dense(items),
            named: None,
        }
    }

    /// An array of `length` whose present elements are `present` (indices below `length`;
    /// a later one for the same index wins) and whose other own properties are `named` (no
    /// array index or `length` among them).
    pub fn from_parts(
        length: u32,
        present: impl IntoIterator<Item = (u32, Value)>,
        named: IndexMap<String, Value>,
    ) -> Self {
        let mut map: BTreeMap<u32, Value> = BTreeMap::new();
        for (i, v) in present {
            if i < length {
                map.insert(i, v);
            }
        }
        let elements = if map.len() == length as usize {
            Elements::Dense(map.into_values().collect())
        } else {
            Elements::Sparse {
                length,
                present: map,
            }
        };
        Array {
            elements,
            named: (!named.is_empty()).then(|| Box::new(named)),
        }
    }

    /// `length`.
    pub fn len(&self) -> usize {
        match &self.elements {
            Elements::Dense(items) => items.len(),
            Elements::Sparse { length, .. } => *length as usize,
        }
    }

    /// `length === 0`.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The element at `index`, if it is present (`None` for a hole or past the end).
    pub fn get(&self, index: usize) -> Option<&Value> {
        match &self.elements {
            Elements::Dense(items) => items.get(index),
            Elements::Sparse { present, .. } => {
                u32::try_from(index).ok().and_then(|i| present.get(&i))
            }
        }
    }

    /// Is every index below the length present (no hole)?
    pub fn is_dense(&self) -> bool {
        matches!(self.elements, Elements::Dense(_))
    }

    /// The elements when there is no hole.
    pub fn as_dense(&self) -> Option<&[Value]> {
        match &self.elements {
            Elements::Dense(items) => Some(items),
            Elements::Sparse { .. } => None,
        }
    }

    /// The elements present, by ascending index (what `Object.entries`, `map`, `flatMap`,
    /// `reduce` and `forEach` visit; a hole is skipped).
    pub fn present(&self) -> Present<'_> {
        match &self.elements {
            Elements::Dense(items) => Present::Dense(items.iter().enumerate()),
            Elements::Sparse { present, .. } => Present::Sparse(present.iter()),
        }
    }

    /// How many elements are present.
    pub fn present_len(&self) -> usize {
        match &self.elements {
            Elements::Dense(items) => items.len(),
            Elements::Sparse { present, .. } => present.len(),
        }
    }

    /// The own properties that are not elements or `length`, in the order they were set.
    pub fn named(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.named.iter().flat_map(|n| n.iter())
    }

    /// Has the array own properties besides its elements and `length`?
    pub fn has_named(&self) -> bool {
        self.named.as_ref().is_some_and(|n| !n.is_empty())
    }

    /// An own property that is not an element or `length`.
    pub fn named_get(&self, key: &str) -> Option<&Value> {
        self.named.as_ref().and_then(|n| n.get(key))
    }

    /// The elements as a list, a hole as `None` (for a length that fits memory: the caller
    /// checks [`Array::len`]).
    pub fn to_options(&self) -> Vec<Option<Value>> {
        match &self.elements {
            Elements::Dense(items) => items.iter().cloned().map(Some).collect(),
            Elements::Sparse { length, present } => {
                let mut out = vec![None; *length as usize];
                for (i, v) in present {
                    out[*i as usize] = Some(v.clone());
                }
                out
            }
        }
    }

    /// Takes every value out (elements, then the others), leaving the array empty.
    pub(super) fn take_values(&mut self) -> Vec<Value> {
        let mut out = match std::mem::take(&mut self.elements) {
            Elements::Dense(items) => items,
            Elements::Sparse { present, .. } => present.into_values().collect(),
        };
        if let Some(named) = self.named.take() {
            out.extend(named.into_values());
        }
        out
    }

    /// The elements of a dense array without other properties, as a list (else `None`).
    pub fn into_vec(mut self) -> Option<Vec<Value>> {
        if self.has_named() {
            return None;
        }
        match std::mem::take(&mut self.elements) {
            Elements::Dense(items) => Some(items),
            sparse @ Elements::Sparse { .. } => {
                self.elements = sparse;
                None
            }
        }
    }
}

/// The present elements of an [`Array`], by ascending index.
pub enum Present<'a> {
    Dense(std::iter::Enumerate<std::slice::Iter<'a, Value>>),
    Sparse(std::collections::btree_map::Iter<'a, u32, Value>),
}

impl<'a> Iterator for Present<'a> {
    type Item = (u32, &'a Value);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Present::Dense(items) => items.next().map(|(i, v)| (i as u32, v)),
            Present::Sparse(present) => present.next().map(|(i, v)| (*i, v)),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Present::Dense(items) => items.size_hint(),
            Present::Sparse(present) => present.size_hint(),
        }
    }
}

impl From<Vec<Value>> for Array {
    fn from(items: Vec<Value>) -> Self {
        Array::new(items)
    }
}
