//! Offline current-geometry oracle. Bodies own ordinary surface rows only in
//! this reference; the measured candidate retains the receiver/media model.
use super::*;
use ara_render::instanced::transport::{BodyClass, NativeChangeFeed};

pub(super) struct GeometryReference {
    pub transport: PatchTransport,
    feed: NativeChangeFeed,
    layout: (usize, usize),
}

impl GeometryReference {
    pub fn new(f: &Fixture, frame: u64, global: bool, out: &Path) -> Result<Self> {
        let transport = PatchTransport::new(
            &f.scene,
            &f.textures,
            BTreeMap::new(),
            TransportOptions {
                global_directions: global,
                ..TransportOptions::default()
            },
            Some(&out.join("geometry-charts")),
        )?;
        let mut feed = NativeChangeFeed::new(&f.scene, frame)?;
        declare_surfaces(&mut feed, f)?;
        feed.finish();
        Ok(Self {
            transport,
            feed,
            layout: (f.scene.geometry.len(), f.scene.instances.len()),
        })
    }

    pub fn update(&mut self, f: &Fixture, frame: u64) -> Result<()> {
        self.feed.begin_frame(frame)?;
        let layout = (f.scene.geometry.len(), f.scene.instances.len());
        if layout != self.layout {
            self.feed.replace(&f.scene)?;
            declare_surfaces(&mut self.feed, f)?;
            self.layout = layout;
        }
        self.feed.update(&f.scene, None)?;
        self.transport
            .advance(&f.scene, &f.textures, self.feed.finish(), true)?;
        Ok(())
    }
}

fn declare_surfaces(feed: &mut NativeChangeFeed, f: &Fixture) -> Result<()> {
    let mut hints = ara_render::instanced::Changes::default();
    // Explicit declarations prevent moving reference surfaces being inferred as
    // kinematic receivers. This is deliberately an expensive offline oracle.
    hints.transport.bodies = (0..f.scene.instances.len())
        .map(|row| (row, BodyClass::Static))
        .collect();
    feed.update(&f.scene, Some(&hints))
}
