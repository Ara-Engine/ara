use anyhow::{Result,ensure};
use ara_gpu::{Access,Barrier,Device,Resource,commands::Command,vk,vulkan::VulkanDevice};
use ara_vfx::Particle;
use glam::{Vec3,Vec4};
use crate::GpuSystem;

impl GpuSystem {
    /// Diagnostic SoA readback, including dead slots. This stalls for completion;
    /// gameplay must consume CpuSystem instead of cosmetic GPU particle state.
    pub fn read_particles(&mut self,gpu:&mut VulkanDevice) -> Result<Vec<Vec<Particle>>> {
        ensure!(self.tick>0,"step before reading particle state");
        let size=u64::from(self.layout.total)*64;
        let staging=gpu.create_buffer(size,vk::BufferUsageFlags::TRANSFER_DST,true)?;
        let previous=self.resources[0].state;
        let mut recorder=gpu.begin(previous.queue);
        recorder.push(Command::Barrier(Barrier {resource:Resource::Buffer(self.particles),before:previous.access,after:Access::CopySource,handoff:None}));
        recorder.push(Command::Barrier(Barrier {resource:Resource::Buffer(staging),before:Access::Undefined,after:Access::CopyDestination,handoff:None}));
        recorder.push(Command::CopyBuffer {source:self.particles,destination:staging,size});
        recorder.push(Command::Barrier(Barrier {resource:Resource::Buffer(self.particles),before:Access::CopySource,after:previous.access,handoff:None}));
        recorder.push(Command::Barrier(Barrier {resource:Resource::Buffer(staging),before:Access::CopyDestination,after:Access::HostRead,handoff:None}));
        let point=gpu.submit(recorder.finish(),&[])?;gpu.resolve(point)?;let bytes=gpu.read(staging)?;
        gpu.retire(Resource::Buffer(staging))?;gpu.collect()?;
        let words=bytes.as_chunks::<4>().0.iter().map(|b|u32::from_le_bytes(*b)).collect::<Vec<_>>();
        let n=self.layout.total as usize;
        let lane=|slot:usize,channel:usize,i:usize|f32::from_bits(words[(slot+channel*n)*4+i]);
        Ok(self.layout.configs.iter().map(|c|(0..c.partition[1] as usize).map(|i|{
            let slot=c.partition[0] as usize+i;
            Particle {position:Vec3::new(lane(slot,0,0),lane(slot,0,1),lane(slot,0,2)),age:lane(slot,0,3),velocity:Vec3::new(lane(slot,1,0),lane(slot,1,1),lane(slot,1,2)),life:lane(slot,1,3),color:Vec4::new(lane(slot,2,0),lane(slot,2,1),lane(slot,2,2),lane(slot,2,3)),size:lane(slot,3,0),seed:words[(slot+3*n)*4+1],birth:words[(slot+3*n)*4+2]}
        }).collect()).collect())
    }
}
