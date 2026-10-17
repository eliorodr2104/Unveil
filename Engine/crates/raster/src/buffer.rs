//! Modified for Unveil (2026-10-17): explicit storage ownership for selected large decode buffers.

use lightcraft_sysmem::ReturnableVec;
use std::ops::{Deref, DerefMut};

/// Pixels are either adopted without copying or allocated with returnable backing. Both variants
/// own initialized elements, and expose ordinary Rust slices with the owner's borrow lifetime.
#[derive(Clone, Debug)]
pub enum Buffer<T> {
    Heap(Vec<T>),
    Returnable(ReturnableVec<T>),
}

impl<T> Default for Buffer<T> {
    fn default() -> Self {
        Self::Heap(Vec::new())
    }
}

impl<T> From<Vec<T>> for Buffer<T> {
    fn from(data: Vec<T>) -> Self {
        Self::Heap(data)
    }
}

impl<T> FromIterator<T> for Buffer<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::Heap(iter.into_iter().collect())
    }
}

impl<T: Clone> Buffer<T> {
    /// Uses a dedicated mapping for large Apple allocations. Allocation failure has the same
    /// behavior as an ordinary Vec; callers requiring a recoverable error can use ReturnableVec.
    pub fn filled_returnable(len: usize, value: T) -> Self {
        let mut data = ReturnableVec::new_in(lightcraft_sysmem::Returnable);
        data.reserve_exact(len);
        data.resize(len, value);
        Self::Returnable(data)
    }
}

impl<T> Buffer<T> {
    pub fn capacity(&self) -> usize {
        match self {
            Self::Heap(v) => v.capacity(),
            Self::Returnable(v) => v.capacity(),
        }
    }

    pub fn truncate(&mut self, len: usize) {
        match self {
            Self::Heap(v) => v.truncate(len),
            Self::Returnable(v) => v.truncate(len),
        }
    }
}

impl<T> Deref for Buffer<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Self::Heap(v) => v,
            Self::Returnable(v) => v,
        }
    }
}

impl<T> DerefMut for Buffer<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match self {
            Self::Heap(v) => v,
            Self::Returnable(v) => v,
        }
    }
}

impl<T: PartialEq> PartialEq for Buffer<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: PartialEq> PartialEq<Vec<T>> for Buffer<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        &**self == other.as_slice()
    }
}

impl<'a, T> IntoIterator for &'a Buffer<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Buffer<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
