//! Rebase authored X/Z coordinates into the GPU's bounded, zero-based tile grid.
use anyhow::{Result, ensure};
use ara_terrain::{Header, TileCoord};
use glam::Vec3;

pub(crate) struct Space {
    pub min: TileCoord,
    pub offset: Vec3,
    pub tiles: u32,
}

impl Space {
    pub fn new(header: &Header) -> Result<Self> {
        header.validate().map_err(|d| anyhow::anyhow!(d.message))?;
        let width = header.max.x - header.min.x;
        let depth = header.max.z - header.min.z;
        ensure!(
            width == depth,
            "GPU landscape expects square terrain extents"
        );
        ensure!(
            width <= 128 && header.cells == 32,
            "GPU patch ABI requires 32 cells and <=128 tiles per edge"
        );
        let offset = Vec3::new(
            header.origin[0] + header.min.x as f32 * header.tile_width(),
            0.,
            header.origin[1] + header.min.z as f32 * header.tile_width(),
        );
        Ok(Self {
            min: header.min,
            offset,
            tiles: width as u32,
        })
    }

    pub fn local_tile(&self, coord: TileCoord) -> Result<TileCoord> {
        let x = i64::from(coord.x) - i64::from(self.min.x);
        let z = i64::from(coord.z) - i64::from(self.min.z);
        ensure!(
            (0..i64::from(self.tiles)).contains(&x) && (0..i64::from(self.tiles)).contains(&z),
            "streamed GPU tile is outside the terrain extent"
        );
        Ok(TileCoord {
            x: x as i32,
            z: z as i32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_origin_and_signed_tile_bounds_share_one_local_space() {
        let header = Header {
            origin: [125., -300.],
            spacing: 2.,
            cells: 32,
            min: TileCoord { x: -2, z: 3 },
            max: TileCoord { x: 2, z: 7 },
            materials: Vec::new(),
        };
        let space = Space::new(&header).expect("valid space");
        assert_eq!(space.tiles, 4);
        assert_eq!(space.offset, Vec3::new(-3., 0., -108.));
        assert_eq!(
            space.local_tile(header.min).expect("minimum"),
            TileCoord { x: 0, z: 0 }
        );
        assert_eq!(
            space.local_tile(TileCoord { x: 1, z: 6 }).expect("last"),
            TileCoord { x: 3, z: 3 }
        );
        assert!(space.local_tile(header.max).is_err());
        assert!(
            space
                .local_tile(TileCoord {
                    x: i32::MAX,
                    z: i32::MIN
                })
                .is_err()
        );
        let world_eye = Vec3::new(10., 25., -80.);
        let world_target = Vec3::new(25., 12., -50.);
        let local_eye = world_eye - space.offset;
        let local_target = world_target - space.offset;
        assert_eq!(world_target - world_eye, local_target - local_eye);
        assert_eq!(local_eye.y, world_eye.y);
    }
}
