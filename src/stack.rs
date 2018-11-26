pub struct RefillingStack<T> {
    base: T,
    variants: Vec<T>
}

impl<T: Clone> RefillingStack<T> {
    pub fn new(base: T) -> RefillingStack<T> {
        RefillingStack {
            base: base,
            variants: Vec::new()
        }
    }

    pub fn base(&self) -> &T {
        &self.base
    }

    pub fn top(&self) -> &T {
        self.variants.last().unwrap_or(&self.base)
    }

    pub fn top_mut(&mut self) -> &mut T {
        if self.variants.is_empty() {
            self.variants.push(self.base.clone());
        }
        self.variants.last_mut().unwrap()
    }

    pub fn duplicate_top(&mut self) {
        self.push(self.top().clone());
    }

    pub fn push(&mut self, value: T) {
        self.variants.push(value);
    }

    pub fn pop(&mut self) {
        self.variants.pop();
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.base).chain(self.variants.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        std::iter::once(&mut self.base).chain(self.variants.iter_mut())
    }
}
