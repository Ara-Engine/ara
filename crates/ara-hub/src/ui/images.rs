//! Pictures for the Hub's views: project and template thumbnails (decoded off the UI thread,
//! centre-cropped to 16:9), generated covers for those without one, and the brand marks.
//! Views ask with [`Images::get`] (it remembers what was missing); the host starts the work
//! ([`Images::start`]) and lands results ([`Images::receive`]), which re-runs the view.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use ara_kit::look::Look;
use stillkit::Image;

/// Every cover and thumbnail is this size in device px (16:9), scaled into its box.
pub const W: u32 = 640;
/// Height of covers and thumbnails.
pub const H: u32 = 360;

/// A picture a view can ask for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pic {
    /// A thumbnail file.
    File(PathBuf),
    /// A generated cover for a seed, in the dark or light scheme.
    Cover(u64, bool),
    /// A brand mark at a size (device px, square).
    Brand(Brand, u32),
}

/// Brand marks the Hub shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Brand {
    /// The Hub's icon.
    Hub,
    /// The engine's icon.
    Engine,
}

impl Brand {
    fn source(self) -> &'static str {
        match self {
            Brand::Hub => include_str!("../../../../assets/brand/icons/hub/ara-hub-small.svg"),
            Brand::Engine => {
                include_str!("../../../../assets/brand/icons/engine/ara-engine-small.svg")
            }
        }
    }
}

/// A finished picture, or `None` when it couldn't be made.
type Made = (Pic, Option<Image>);

/// Decoded pictures by what they show.
#[derive(Default)]
pub struct Images {
    ready: HashMap<Pic, Arc<Image>>,
    /// Asked for and not ready: started, or waiting to start.
    wanted: RefCell<HashSet<Pic>>,
    started: HashSet<Pic>,
    failed: HashSet<Pic>,
    results: Option<(Sender<Made>, Receiver<Made>)>,
}

impl Images {
    /// The picture if it is ready; otherwise `None`, and the host will make it.
    pub fn get(&self, pic: &Pic) -> Option<Arc<Image>> {
        if let Some(image) = self.ready.get(pic) {
            return Some(image.clone());
        }
        if !self.failed.contains(pic) {
            self.wanted.borrow_mut().insert(pic.clone());
        }
        None
    }

    /// Whether a picture failed to load (its fallback should show).
    pub fn failed(&self, pic: &Pic) -> bool {
        self.failed.contains(pic)
    }

    /// Starts making what views asked for, on worker threads; `wake` runs as each lands.
    pub fn start(&mut self, look: &Look, wake: impl Fn() + Clone + Send + 'static) {
        let wanted: Vec<Pic> = self
            .wanted
            .borrow_mut()
            .drain()
            .filter(|p| !self.started.contains(p))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let send = self.results.get_or_insert_with(channel).0.clone();
        for pic in wanted {
            self.started.insert(pic.clone());
            let (send, wake, look) = (send.clone(), wake.clone(), look.clone());
            std::thread::spawn(move || {
                let image = make(&pic, &look);
                let _ = send.send((pic, image));
                wake();
            });
        }
    }

    /// Makes everything asked for now, on this thread (headless captures).
    pub fn load_now(&mut self, look: &Look) -> bool {
        let wanted: Vec<Pic> = self.wanted.borrow_mut().drain().collect();
        let any = !wanted.is_empty();
        for pic in wanted {
            let image = make(&pic, look);
            self.land(pic, image);
        }
        any
    }

    /// Lands finished work; whether anything new is ready (run the view again).
    pub fn receive(&mut self) -> bool {
        let Some((_, results)) = &self.results else {
            return false;
        };
        let done: Vec<_> = results.try_iter().collect();
        let any = !done.is_empty();
        for (pic, image) in done {
            self.land(pic, image);
        }
        any
    }

    fn land(&mut self, pic: Pic, image: Option<Image>) {
        self.started.remove(&pic);
        match image {
            Some(image) => {
                self.ready.insert(pic, Arc::new(image));
            }
            None => {
                self.failed.insert(pic);
            }
        }
    }

    /// Forgets generated covers of the other scheme (after a theme switch).
    pub fn keep_scheme(&mut self, dark: bool) {
        self.ready
            .retain(|pic, _| !matches!(pic, Pic::Cover(_, d) if *d != dark));
    }
}

fn make(pic: &Pic, look: &Look) -> Option<Image> {
    match pic {
        Pic::Cover(seed, dark) => Some(ara_kit::art::cover(&Look::new(*dark), *seed, W, H)),
        Pic::Brand(brand, px) => ara_kit::art::svg(brand.source(), *px, *px),
        Pic::File(path) => {
            let _ = look;
            let decoded = image::open(path).ok()?.to_rgba8();
            Some(crop(&decoded))
        }
    }
}

/// Centre-crops to 16:9 and scales to [`W`] x [`H`], as opaque BGRA.
fn crop(src: &image::RgbaImage) -> Image {
    let (sw, sh) = (src.width().max(1) as f32, src.height().max(1) as f32);
    let target = W as f32 / H as f32;
    let (cw, ch) = if sw / sh > target {
        (sh * target, sh)
    } else {
        (sw, sw / target)
    };
    let (ox, oy) = ((sw - cw) / 2.0, (sh - ch) / 2.0);
    let view =
        image::imageops::crop_imm(src, ox as u32, oy as u32, cw as u32, ch as u32).to_image();
    let scaled = image::imageops::resize(&view, W, H, image::imageops::FilterType::Triangle);
    let mut pixels = Vec::with_capacity((W * H * 4) as usize);
    for p in scaled.pixels() {
        pixels.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    Image {
        width: W,
        height: H,
        pixels,
    }
}
