//! Small JSON objects avoid a heap allocation; wide record maps retain hashed lookup.
use std::{borrow::Cow, collections::HashSet};

#[derive(Default)]
pub(super) struct Keys<'a> {
    small: [Option<Cow<'a, str>>; 4],
    count: usize,
    large: Option<HashSet<Cow<'a, str>>>,
}

impl<'a> Keys<'a> {
    pub(super) fn insert(&mut self, key: Cow<'a, str>) -> bool {
        if let Some(keys) = &mut self.large {
            return keys.insert(key);
        }
        if self.small[..self.count]
            .iter()
            .flatten()
            .any(|previous| previous == &key)
        {
            return false;
        }
        if self.count < self.small.len() {
            self.small[self.count] = Some(key);
            self.count += 1;
            return true;
        }
        let mut keys = HashSet::with_capacity(self.small.len() * 2);
        for previous in self.small.iter_mut().filter_map(Option::take) {
            keys.insert(previous);
        }
        let inserted = keys.insert(key);
        self.large = Some(keys);
        inserted
    }
}
