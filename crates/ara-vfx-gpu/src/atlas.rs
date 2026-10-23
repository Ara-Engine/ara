//! Deterministic offline sprite-sheet generation; no texture synthesis at runtime.
use ara_gpu::{Access, Barrier, Device, Image, Queue, Resource, commands::Command, vk, vulkan::VulkanDevice};
use anyhow::{Result, ensure};

/// One padded tile is 128 pixels; four by four frames per texture family.
pub const ATLAS_SIZE: [u32; 2] = [512, 2560];

fn hash(x: i32, y: i32) -> f32 {
    let mut n = (x as u32).wrapping_mul(0x1f123bb5) ^ (y as u32).wrapping_mul(0x5f356495);
    n ^= n >> 16; n = n.wrapping_mul(0x7feb352d); n ^= n >> 15;
    (n >> 8) as f32 / 16777216.
}
fn noise(x: f32, y: f32) -> f32 {
    let ix=x.floor() as i32; let iy=y.floor() as i32;
    let a=x-x.floor(); let b=y-y.floor();
    let a=a*a*(3.-2.*a); let b=b*b*(3.-2.*b);
    let u=hash(ix,iy)*(1.-a)+hash(ix+1,iy)*a;
    let v=hash(ix,iy+1)*(1.-a)+hash(ix+1,iy+1)*a;
    u*(1.-b)+v*b
}
fn fbm(mut x:f32, mut y:f32) -> f32 {
    let mut sum=0.; let mut weight=0.55;
    for _ in 0..5 {sum+=noise(x,y)*weight; x=x*2.03+13.1; y=y*2.03-7.7; weight*=0.48;}
    sum
}
fn density(kind:u32, x:f32, y:f32, time:f32) -> f32 {
    let radius=(x*x+y*y).sqrt();
    match kind {
        0 => { // Rising flame: the top narrows and tears into advected tongues.
            let h=(y+1.)*0.5;
            let turbulence=fbm(x*3.+fbm(x*2.,h*4.-time)*2.,h*5.-time*2.);
            let width=(1.-h).max(0.).powf(0.7)*0.82;
            let body=(1.-(x+(turbulence-0.5)*0.65).abs()/width.max(0.02)).clamp(0.,1.);
            (body.powf(0.65)*(turbulence*1.6-0.2).max(0.)*(h*12.).min(1.)*((1.-h)*10.).min(1.)).clamp(0.,1.)
        }
        1 => {let n=fbm(x*3.2+time*0.17,y*3.2-time*0.55); ((1.-radius).max(0.)*2.5*(n*1.8-0.25).max(0.)).clamp(0.,1.)}
        2 => ((-x*x*14.-y*y*2.8).exp()*(1.-radius).max(0.)*3.).clamp(0.,1.),
        3 => {let r=0.25+time/8.*0.5; let w=0.055; (-((radius-r)/w).powi(2)).exp()*(1.-radius).max(0.)},
        _ => {let angle=y.atan2(x); let path=0.5+0.12*(angle*2.+time).sin(); let band=(-((radius-path)/0.07).powi(2)).exp(); band*(0.3+0.7*fbm(x*5.,y*5.+time))}
    }
}
/// Cook five original 16-frame sheets into one linear RGBA8 atlas. R stores
/// density, G/B its lighting gradients and A smooth opacity; no third-party media.
pub fn cook_atlas() -> Vec<u8> {
    let mut pixels=vec![0u8;(ATLAS_SIZE[0]*ATLAS_SIZE[1]*4) as usize];
    for kind in 0..5 {for frame in 0..16 {for py in 0..128 {for px in 0..128 {
        // A two-pixel transparent gutter prevents neighboring-frame filtering.
        if !(2..126).contains(&px) || !(2..126).contains(&py) {continue;}
        let x=(px as f32-63.5)/61.; let y=(py as f32-63.5)/61.;
        let time=frame as f32*0.5;
        let d=density(kind,x,y,time);
        let gx=(density(kind,x+0.018,y,time)-density(kind,x-0.018,y,time))*3.;
        let gy=(density(kind,x,y+0.018,time)-density(kind,x,y-0.018,time))*3.;
        let at=(((kind*4+frame/4)*128+py)*ATLAS_SIZE[0]+(frame%4)*128+px)*4;
        let rgba=[d,0.5+gx*0.5,0.5+gy*0.5,d];
        for c in 0..4 {pixels[at as usize+c]=(rgba[c].clamp(0.,1.)*255.).round() as u8;}
    }}}}
    pixels
}
pub(crate) fn upload(gpu:&mut VulkanDevice, bytes:&[u8]) -> Result<Image> {
    ensure!(bytes.len()==(ATLAS_SIZE[0]*ATLAS_SIZE[1]*4) as usize,"invalid cooked sprite atlas");
    let stage=gpu.create_buffer(bytes.len() as u64,vk::BufferUsageFlags::TRANSFER_SRC,true)?;
    gpu.upload(stage,bytes)?;
    let image=gpu.create_image(ATLAS_SIZE[0],ATLAS_SIZE[1],vk::Format::R8G8B8A8_UNORM,vk::ImageUsageFlags::SAMPLED|vk::ImageUsageFlags::TRANSFER_DST)?;
    let mut c=gpu.begin(Queue::Graphics);
    c.barriers([Barrier{resource:Resource::Buffer(stage),before:Access::HostWrite,after:Access::CopySource,handoff:None},Barrier{resource:Resource::Image(image),before:Access::Undefined,after:Access::CopyDestination,handoff:None}]);
    c.push(Command::UploadImage{source:stage,destination:image});
    c.barriers([Barrier{resource:Resource::Image(image),before:Access::CopyDestination,after:Access::ShaderRead,handoff:None}]);
    let p=gpu.submit(c.finish(),&[])?;gpu.resolve(p)?;gpu.retire(Resource::Buffer(stage))?;gpu.collect()?;
    Ok(image)
}
