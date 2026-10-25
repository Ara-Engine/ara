//! Completed timestamp pools stay reusable; in-flight pools live in Pending.
use ash::vk;

pub(super) const MAX_CACHED_POOLS: usize = 32;
pub(super) const MAX_CACHED_QUERIES: u32 = 1024;

pub(super) struct Pool {
    pub handle: vk::QueryPool,
    pub capacity: u32,
}

pub(super) struct Cache {
    available: Vec<Pool>,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            available: Vec::with_capacity(MAX_CACHED_POOLS),
        }
    }
}
impl Cache {
    pub fn take(&mut self, required: u32) -> Option<Pool> {
        let index = self
            .available
            .iter()
            .enumerate()
            .filter(|(_, pool)| pool.capacity >= required)
            .min_by_key(|(_, pool)| pool.capacity)
            .map(|(i, _)| i)?;
        Some(self.available.swap_remove(index))
    }

    /// Takes completed ownership; returns the rejected/evicted pool to destroy.
    pub fn recycle(&mut self, pool: Pool) -> Option<Pool> {
        if pool.capacity > MAX_CACHED_QUERIES {
            return Some(pool);
        }
        if self.available.len() < MAX_CACHED_POOLS {
            self.available.push(pool);
            return None;
        }
        let Some(smallest) = self.available.iter_mut().min_by_key(|entry| entry.capacity) else {
            return Some(pool);
        };
        if smallest.capacity < pool.capacity {
            Some(std::mem::replace(smallest, pool))
        } else {
            Some(pool)
        }
    }

    pub fn drain(&mut self) -> std::vec::Drain<'_, Pool> {
        self.available.drain(..)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.available.len()
    }
}
