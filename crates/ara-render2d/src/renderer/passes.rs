use super::*;
impl Renderer2d {
    pub(super) fn render_mode(
        &mut self,
        camera: &Camera2d,
        exposure: f32,
        mode: ViewMode,
    ) -> Result<()> {
        ensure!(
            matches!(
                mode,
                ViewMode::Lit
                    | ViewMode::Albedo
                    | ViewMode::Normals
                    | ViewMode::EntityIds
                    | ViewMode::ShadowCascades
            ),
            "unsupported 2D capture mode"
        );
        let (scale, centre) = camera.resolved()?;
        let view = [
            centre[0],
            centre[1],
            2. * scale / self.size[0] as f32,
            2. * scale / self.size[1] as f32,
        ];
        self.scene_frame[..16].copy_from_slice(bytemuck::bytes_of(&view));
        self.scene_frame[16..32].copy_from_slice(bytemuck::bytes_of(&[
            camera.ambient[0],
            camera.ambient[1],
            camera.ambient[2],
            1. / camera.pixels_per_metre,
        ]));
        self.scene_frame[40..44].copy_from_slice(&(mode as u32).to_le_bytes());
        self.scene_frame[44..48].copy_from_slice(&u32::from(camera.pixel_perfect).to_le_bytes());
        self.gpu.upload(self.frame, &self.scene_frame)?;
        self.post.prepare(
            &self.gpu,
            self.size,
            exposure,
            self.output_index,
            matches!(mode, ViewMode::Lit),
        )?;
        let state = |access| State {
            access,
            queue: Queue::Graphics,
        };
        let mut graph = Graph::default();
        let targets: Vec<_> = self
            .targets
            .iter()
            .zip(self.states)
            .map(|(&i, s)| graph.import(Resource::Image(i), s))
            .collect();
        let frame = graph.import(Resource::Buffer(self.frame), state(Access::HostWrite));
        let post_frame = graph.import(
            Resource::Buffer(self.post.frame_buffer()),
            state(Access::HostWrite),
        );
        let instances = graph.import(Resource::Buffer(self.instances), state(Access::ShaderRead));
        let mut reads = vec![
            (targets[0], Access::Color),
            (targets[1], Access::Color),
            (frame, Access::ShaderRead),
            (instances, Access::ShaderRead),
        ];
        for &image in &self.images {
            reads.push((
                graph.import(Resource::Image(image), state(Access::ShaderRead)),
                Access::ShaderRead,
            ));
        }
        graph.pass("Sprites.TileChunks.Bindless", Queue::Graphics, reads);
        graph.pass(
            "Exposure.ACESFitted.SDR",
            Queue::Graphics,
            [
                (targets[0], Access::ShaderRead),
                (targets[2], Access::Storage),
                (post_frame, Access::ShaderRead),
            ],
        );
        let readbacks: Vec<_> = self
            .readbacks
            .iter()
            .map(|&b| graph.import(Resource::Buffer(b), state(Access::Undefined)))
            .collect();
        graph.pass(
            "CaptureReadback",
            Queue::Graphics,
            [
                (targets[2], Access::CopySource),
                (targets[1], Access::CopySource),
                (readbacks[0], Access::CopyDestination),
                (readbacks[1], Access::CopyDestination),
            ],
        );
        graph.pass(
            "HostReadback",
            Queue::Graphics,
            readbacks.iter().map(|id| (*id, Access::HostRead)),
        );
        let compiled = graph.compile()?;
        let mut recorder = self.gpu.begin(Queue::Graphics);
        for pass in &compiled.passes {
            recorder.barriers(pass.before.iter().copied());
            recorder.push(Command::BeginPass(pass.name.to_owned()));
            match pass.id.0 {
                0 => {
                    recorder.push(Command::Pipeline(self.pipeline));
                    recorder.push(Command::BeginRendering {
                        colors: vec![
                            Attachment {
                                image: self.targets[0],
                                clear: Some([0.012, 0.018, 0.032, 1.]),
                            },
                            Attachment {
                                image: self.targets[1],
                                clear: Some([0.; 4]),
                            },
                        ],
                        depth: None,
                        size: self.size,
                    });
                    for batch in &self.batches {
                        if let Some((b, p)) = batch.bounds {
                            let x = centre[0] * p[0];
                            let y = centre[1] * p[1];
                            let w = self.size[0] as f32 / scale * 0.5;
                            let h = self.size[1] as f32 / scale * 0.5;
                            if b[2] < x - w || b[0] > x + w || b[3] < y - h || b[1] > y + h {
                                continue;
                            }
                        }
                        recorder.push(Command::Constants(RootConstants {
                            words: [
                                self.frame_index,
                                self.instance_index,
                                batch.first,
                                0,
                                0,
                                0,
                                0,
                                0,
                            ],
                        }));
                        recorder.push(Command::DrawProcedural {
                            vertices: batch.vertices,
                            instances: batch.count,
                        });
                    }
                    recorder.push(Command::EndRendering);
                }
                1 => self.post.record(&mut recorder, self.hdr_index, self.size),
                2 => {
                    recorder.push(Command::ReadImage {
                        source: self.targets[2],
                        destination: self.readbacks[0],
                    });
                    recorder.push(Command::ReadImage {
                        source: self.targets[1],
                        destination: self.readbacks[1],
                    });
                }
                _ => {}
            }
            recorder.push(Command::EndPass);
            recorder.barriers(pass.after.iter().copied());
        }
        let point = self.gpu.submit(recorder.finish(), &[])?;
        self.timings = self.gpu.resolve(point)?;
        for (i, id) in targets.iter().enumerate() {
            self.states[i] = compiled.final_states[id.0];
        }
        self.gpu.assert_clean()?;
        Ok(())
    }
}
