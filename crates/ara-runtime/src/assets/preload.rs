//! Bounded control-boundary preparation, with deterministic publication/error order.
use super::*;

#[cfg(test)]
mod tests;

fn workers(count: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4)
        .min(count.max(1))
}

fn batch(source: &dyn AssetSource, ids: &[&str]) -> (Models, Vec<RuntimeError>) {
    let mut models = Models::default();
    let mut errors = Vec::new();
    // Only one cooked generation is decoded per worker at a time. The temporary
    // import payload drops before the next generation; published models stay shared.
    for &id in ids {
        if let Err(error) = models.load(source, id) {
            errors.push(error);
        }
    }
    (models, errors)
}

impl Models {
    pub(crate) fn preload(
        source: &dyn AssetSource,
        ids: &BTreeSet<String>,
    ) -> Result<(Self, Vec<RuntimeError>)> {
        let ids: Vec<_> = ids.iter().map(String::as_str).collect();
        let workers = workers(ids.len());
        if workers == 1 {
            return Ok(batch(source, &ids));
        }
        let batches = std::thread::scope(|scope| {
            let jobs: Vec<_> = ids
                .chunks(ids.len().div_ceil(workers))
                .map(|ids| scope.spawn(move || batch(source, ids)))
                .collect();
            // Join every job before inspecting failures, including when a source
            // panics. Results stay in sorted input order, never completion order.
            jobs.into_iter().map(|job| job.join()).collect::<Vec<_>>()
        });
        let mut models = Self::default();
        let mut errors = Vec::new();
        for result in batches {
            let (mut prepared, failed) =
                result.map_err(|_| RuntimeError("model preload worker panicked".into()))?;
            models.models.append(&mut prepared.models);
            models.wanted.append(&mut prepared.wanted);
            errors.extend(failed);
        }
        Ok((models, errors))
    }
}
