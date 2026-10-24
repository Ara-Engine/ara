//! Consecutive passes on one queue share recording storage and a timeline signal.
use crate::graph::CompiledPass;
use ara_gpu::TimelinePoint;

pub(super) fn batches(passes: &[CompiledPass]) -> impl Iterator<Item = &[CompiledPass]> {
    passes.chunk_by(|a, b| a.queue == b.queue)
}

/// Every cross-queue predecessor is in an earlier batch. One wait at the highest
/// value per queue covers all its earlier signals, including repeated pass aliases.
pub(super) fn waits(batch: &[CompiledPass], points: &[TimelinePoint]) -> Vec<TimelinePoint> {
    let mut waits: Vec<TimelinePoint> = Vec::with_capacity(2);
    for &index in batch.iter().flat_map(|pass| &pass.waits) {
        let point = points[index];
        if let Some(prior) = waits.iter_mut().find(|prior| prior.queue == point.queue) {
            prior.value = prior.value.max(point.value);
        } else {
            waits.push(point);
        }
    }
    waits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Graph;
    use ara_gpu::Queue;

    #[test]
    fn cross_queue_edges_share_batch_signals_without_duplicate_semaphore_waits() {
        let mut graph = Graph::default();
        let a = graph.pass("clear", Queue::Graphics, []);
        let b = graph.pass("draw", Queue::Graphics, []);
        let c = graph.pass("depth", Queue::Graphics, []);
        let d = graph.pass("cull", Queue::Compute, []);
        let e = graph.pass("shade", Queue::Graphics, []);
        graph.after(b, a).unwrap();
        graph.after(c, b).unwrap();
        for source in [a, b, c] {
            graph.after(d, source).unwrap();
        }
        graph.after(e, d).unwrap();
        let compiled = graph.compile().unwrap();
        let batches: Vec<_> = batches(&compiled.passes).collect();
        assert_eq!(
            batches.iter().map(|batch| batch.len()).collect::<Vec<_>>(),
            [3, 1, 1]
        );
        let graphics = TimelinePoint {
            queue: Queue::Graphics,
            value: 7,
        };
        let compute = TimelinePoint {
            queue: Queue::Compute,
            value: 4,
        };
        assert!(waits(batches[0], &[]).is_empty());
        assert_eq!(waits(batches[1], &[graphics; 3]), [graphics]);
        assert_eq!(
            waits(batches[2], &[graphics, graphics, graphics, compute]),
            [compute]
        );
    }
}
