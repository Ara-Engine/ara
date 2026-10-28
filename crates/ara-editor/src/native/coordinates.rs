//! SDL window coordinates, editor logical pixels and native physical pixels are distinct.
use ara_platform::Event;
pub(super) fn logical_event(event: Event, pixel_density: f32, dpi: f32) -> Event {
    let scale = pixel_density / dpi;
    match event {
        Event::MouseMotion { position, delta } => Event::MouseMotion {
            position: position.map(|v| v * scale),
            delta: delta.map(|v| v * scale),
        },
        Event::FileDrop { path, position } => Event::FileDrop {
            path,
            position: position.map(|value| value * scale),
        },
        other => other,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_scale_and_backbuffer_density_are_independent() {
        for (density, dpi, expected) in [
            (1., 2., [80., 50.]),
            (2., 2., [160., 100.]),
            (1., 1., [160., 100.]),
        ] {
            let Event::MouseMotion { position, delta } = logical_event(
                Event::MouseMotion {
                    position: [160., 100.],
                    delta: [4., 2.],
                },
                density,
                dpi,
            ) else {
                panic!("motion event");
            };
            assert_eq!(position, expected);
            assert_eq!(delta, [4. * density / dpi, 2. * density / dpi]);
        }
    }
}
