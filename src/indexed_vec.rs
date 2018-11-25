use std::iter::{Extend, FromIterator};
use std::marker::PhantomData;
use std::slice;
use std::ops::{Index, IndexMut};

#[derive(Clone)]
pub struct IndexVec<I, T> {
    inner: Vec<T>,
    _marker: PhantomData<fn(I) -> T>
}

pub trait Idx: Copy {
    fn new(usize) -> Self;
    fn index(self) -> usize;
}

impl<I: Idx, T> Index<I> for IndexVec<I, T> {
    type Output = T;
    fn index(&self, index: I) -> &T {
        &self.inner[index.index()]
    }
}

impl<I: Idx, T> IndexMut<I> for IndexVec<I, T> {
    fn index_mut(&mut self, index: I) -> &mut T {
        &mut self.inner[index.index()]
    }
}

impl<I: Idx, T> FromIterator<T> for IndexVec<I, T> {
    fn from_iter<Iter: IntoIterator<Item = T>>(iter: Iter) -> Self {
        IndexVec::from_vec(Vec::from_iter(iter))
    }
}

impl<I: Idx, T> Extend<T> for IndexVec<I, T> {
    fn extend<Iter: IntoIterator<Item = T>>(&mut self, iter: Iter) {
        self.inner.extend(iter);
    }
}

impl<'a, I: Idx, T> IntoIterator for &'a IndexVec<I, T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, I: Idx, T> IntoIterator for &'a mut IndexVec<I, T> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<I: Idx, T> IndexVec<I, T> {
    pub fn new() -> Self {
        IndexVec {
            inner: Vec::new(),
            _marker: PhantomData
        }
    }

    pub fn from_vec(items: Vec<T>) -> Self {
        IndexVec {
            inner: items,
            _marker: PhantomData
        }
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn push(&mut self, value: T) -> I {
        let new_idx = I::new(self.inner.len());
        self.inner.push(value);
        new_idx
    }

    pub fn iter<'a>(&'a self) -> slice::Iter<'a, T> {
        self.inner.iter()
    }

    pub fn iter_mut<'a>(&'a mut self) -> slice::IterMut<'a, T> {
        self.inner.iter_mut()
    }
}
