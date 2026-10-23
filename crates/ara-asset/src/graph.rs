use std::collections::{BTreeMap, BTreeSet, VecDeque};
/// Queryable asset/source dependency graph; edges point from consumer to input.
#[derive(Debug, Default, Clone)]
pub struct DependencyGraph {
    forward: BTreeMap<String, BTreeSet<String>>,
    reverse: BTreeMap<String, BTreeSet<String>>,
}
impl DependencyGraph {
    /// Replaces outgoing dependencies atomically, removing stale reverse links.
    pub fn replace(
        &mut self,
        asset: impl Into<String>,
        dependencies: impl IntoIterator<Item = String>,
    ) {
        let asset = asset.into();
        if let Some(old) = self.forward.remove(&asset) {
            for d in old {
                if let Some(users) = self.reverse.get_mut(&d) {
                    users.remove(&asset);
                    if users.is_empty() {
                        self.reverse.remove(&d);
                    }
                }
            }
        }
        let deps: BTreeSet<_> = dependencies.into_iter().collect();
        for d in &deps {
            self.reverse
                .entry(d.clone())
                .or_default()
                .insert(asset.clone());
        }
        self.forward.insert(asset, deps);
    }
    /// Inputs directly used by an asset (`who-uses` in consumer-to-input direction).
    pub fn dependencies(&self, asset: &str) -> impl Iterator<Item = &str> {
        self.forward
            .get(asset)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
    /// Direct consumers of an input (`used-by`).
    pub fn users(&self, input: &str) -> impl Iterator<Item = &str> {
        self.reverse
            .get(input)
            .into_iter()
            .flatten()
            .map(String::as_str)
    }
    /// Every transitive consumer, sorted and cycle-safe, excluding the input itself.
    pub fn invalidated_by(&self, input: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::from([input.to_owned()]);
        let mut queue = VecDeque::from([input.to_owned()]);
        while let Some(next) = queue.pop_front() {
            for user in self.users(&next) {
                if seen.insert(user.to_owned()) {
                    queue.push_back(user.to_owned());
                }
            }
        }
        seen.remove(input);
        seen
    }
}
