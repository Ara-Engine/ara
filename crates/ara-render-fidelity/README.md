# ara-render-fidelity

The authored look as numbers a renderer consumes: the frame's environment (sky, atmosphere, fog,
exposure, display), colour grading and tone mapping, the quality presets and their plans. It does
no GPU work. See `AGENTS.md` for the API.

Renderlab draws Ara (`crates/ara-renderlab`). The ray-traced fidelity renderer that used to live
in this crate, with its shaders and cooked pipelines, is deleted.
