//! Attribute-only prepared contexts are owned by one primitive's sequential run.
#[cfg(feature = "prepared-lods")]
use super::Vertex;
#[cfg(not(feature = "prepared-lods"))]
use super::VertexDataAdapter;
use super::topology;
#[cfg(feature = "prepared-lods")]
use ara_asset::Error;
use ara_asset::Result;
#[cfg(not(feature = "prepared-lods"))]
use meshopt::SimplifyOptions;

pub(super) struct Attributes<'a> {
    #[cfg(feature = "prepared-lods")]
    pub vertices: &'a [Vertex],
    #[cfg(not(feature = "prepared-lods"))]
    pub adapter: &'a VertexDataAdapter<'a>,
    pub original: &'a [u32],
    pub values: &'a [f32],
    pub weights: &'a [f32],
    pub topology: Option<&'a topology::Topology>,
    #[cfg(feature = "prepared-lods")]
    pub prepared: Option<ara_meshopt_sys::Prepared>,
}
impl Attributes<'_> {
    pub fn simplify(&mut self, target: usize, limit: f32, error: &mut f32) -> Result<Vec<u32>> {
        let topology = self.topology.expect("static meshes have topology");
        #[cfg(feature = "prepared-lods")]
        {
            if self.prepared.is_none() {
                let positions: Vec<_> = self.vertices.iter().map(|v| v.position).collect();
                self.prepared = Some(
                    ara_meshopt_sys::Prepared::new(
                        self.original,
                        &positions,
                        self.values,
                        self.weights,
                        &topology.locks.iter().map(|v| v.bits()).collect::<Vec<_>>(),
                    )
                    .map_err(|e| Error::Invalid(format!("prepared LOD setup failed: {e}")))?,
                );
            }
            let prepared = self
                .prepared
                .as_mut()
                .expect("attribute context initialized above");
            let (indices, relative_error) = prepared
                .simplify(target, limit)
                .map_err(|e| Error::Invalid(format!("prepared LOD solve failed: {e}")))?;
            *error = relative_error;
            Ok(indices)
        }
        #[cfg(not(feature = "prepared-lods"))]
        {
            Ok(meshopt::simplify_with_attributes_and_locks(
                self.original,
                self.adapter,
                self.values,
                self.weights,
                std::mem::size_of_val(self.weights),
                &topology.locks,
                target,
                limit,
                SimplifyOptions::LockBorder
                    | SimplifyOptions::Permissive
                    | SimplifyOptions::RegularizeLight,
                Some(error),
            ))
        }
    }
}
