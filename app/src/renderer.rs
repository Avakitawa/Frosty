use std::time::Instant;

use crate::settings::AppSettings;
use objc2::runtime::ProtocolObject;
use objc2::{msg_send, rc::Retained};
use objc2_app_kit::NSAutoresizingMaskOptions;
use objc2_foundation::{ns_string, NSRect};
use objc2_metal::{
    MTLBlendFactor, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLPixelFormat, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderPipelineDescriptor, MTLRenderPipelineState,
    MTLResourceOptions,
};
use objc2_metal_kit::MTKView;
use objc2_quartz_core::CALayer;
use winter_physics::{
    BlockedRegion, BrushStroke, CollisionSurface, Particle, ParticleSystem, PhysicsConfig,
    Snowball, Snowman, ACCUMULATION_BINS, MAX_ACCUMULATION_SURFACES,
};

const PARTICLE_CAPACITY: usize = 20_000;
const INITIAL_PARTICLE_COUNT: usize = 9_000;
const GPU_BUFFER_COUNT: usize = 3;
const ACCUMULATION_SUBDIVISIONS: usize = 8;
const SNOW_MESH_VERTICES_PER_SURFACE: usize = ACCUMULATION_BINS * ACCUMULATION_SUBDIVISIONS * 6;
const SNOW_MESH_VERTEX_CAPACITY: usize = SNOW_MESH_VERTICES_PER_SURFACE * MAX_ACCUMULATION_SURFACES;
const SHAPE_VERTEX_CAPACITY: usize = 40_000;
const SHAPE_CIRCLE_SEGMENTS: usize = 18;

#[repr(C)]
#[derive(Clone, Copy)]
struct GpuParticle {
    x: f32,
    y: f32,
    diameter: f32,
    opacity: f32,
    phase: f32,
    rotation: f32,
    depth: f32,
    window_draft: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GpuBlockedRegion {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GpuSnowVertex {
    x: f32,
    y: f32,
}

/// `float2 position; float2 padding; float4 color;` in MSL. The explicit
/// padding keeps the same 32-byte layout on both sides.
#[repr(C)]
#[derive(Clone, Copy)]
struct GpuShapeVertex {
    x: f32,
    y: f32,
    _padding: [f32; 2],
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

#[repr(C, align(8))]
struct FrameUniforms {
    elapsed_time: f32,
    blocked_region_count: u32,
    drawable_width: f32,
    drawable_height: f32,
    snow_suppressed: u32,
    particles_in_windows: u32,
}

pub struct MetalRenderer {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    particle_buffers: Vec<Retained<ProtocolObject<dyn MTLBuffer>>>,
    snow_mesh_buffers: Vec<Retained<ProtocolObject<dyn MTLBuffer>>>,
    blocked_region_buffers: Vec<Retained<ProtocolObject<dyn MTLBuffer>>>,
    shape_buffers: Vec<Retained<ProtocolObject<dyn MTLBuffer>>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    snow_mesh_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    shape_pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    particles: ParticleSystem,
    blocked_regions: Vec<GpuBlockedRegion>,
    snow_mesh_vertices: Vec<GpuSnowVertex>,
    shape_vertices: Vec<GpuShapeVertex>,
    snow_suppressed: bool,
    accumulation_enabled: bool,
    intensity: f32,
    particles_in_windows: bool,
    cursor_tool: u8,
    hovered_snowball: Option<u64>,
    hovered_snowman: Option<u64>,
    last_cursor_position: Option<(f32, f32)>,
    started_at: Instant,
    last_update: Instant,
    next_buffer: usize,
}

impl MetalRenderer {
    pub fn new(seed: u32) -> Result<Self, String> {
        let device = MTLCreateSystemDefaultDevice()
            .ok_or_else(|| "No compatible Metal device was found".to_owned())?;
        let command_queue = device
            .newCommandQueue()
            .ok_or_else(|| "Metal failed to create a command queue".to_owned())?;

        let shader_source = ns_string!(include_str!("../assets/shaders/snow.metal"));
        let library = device
            .newLibraryWithSource_options_error(shader_source, None)
            .map_err(|error| format!("Metal shader compilation failed: {error}"))?;

        let vertex_function = library
            .newFunctionWithName(ns_string!("snow_vertex"))
            .ok_or_else(|| "The compiled Metal library is missing snow_vertex".to_owned())?;
        let fragment_function = library
            .newFunctionWithName(ns_string!("snow_fragment"))
            .ok_or_else(|| "The compiled Metal library is missing snow_fragment".to_owned())?;

        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setVertexFunction(Some(&vertex_function));
        descriptor.setFragmentFunction(Some(&fragment_function));
        let attachments = descriptor.colorAttachments();
        // A render pipeline descriptor always provides color attachment zero.
        let attachment = unsafe { attachments.objectAtIndexedSubscript(0) };
        attachment.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        attachment.setBlendingEnabled(true);
        attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
        attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .map_err(|error| format!("Metal render pipeline creation failed: {error}"))?;

        let mesh_vertex_function = library
            .newFunctionWithName(ns_string!("snow_mesh_vertex"))
            .ok_or_else(|| "The compiled Metal library is missing snow_mesh_vertex".to_owned())?;
        let mesh_fragment_function = library
            .newFunctionWithName(ns_string!("snow_mesh_fragment"))
            .ok_or_else(|| "The compiled Metal library is missing snow_mesh_fragment".to_owned())?;
        let mesh_descriptor = MTLRenderPipelineDescriptor::new();
        mesh_descriptor.setVertexFunction(Some(&mesh_vertex_function));
        mesh_descriptor.setFragmentFunction(Some(&mesh_fragment_function));
        let mesh_attachments = mesh_descriptor.colorAttachments();
        let mesh_attachment = unsafe { mesh_attachments.objectAtIndexedSubscript(0) };
        mesh_attachment.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        mesh_attachment.setBlendingEnabled(true);
        mesh_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
        mesh_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        mesh_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        mesh_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        let snow_mesh_pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&mesh_descriptor)
            .map_err(|error| format!("Snow mesh pipeline creation failed: {error}"))?;

        let shape_vertex_function = library
            .newFunctionWithName(ns_string!("shape_vertex"))
            .ok_or_else(|| "The compiled Metal library is missing shape_vertex".to_owned())?;
        let shape_fragment_function = library
            .newFunctionWithName(ns_string!("shape_fragment"))
            .ok_or_else(|| "The compiled Metal library is missing shape_fragment".to_owned())?;
        let shape_descriptor = MTLRenderPipelineDescriptor::new();
        shape_descriptor.setVertexFunction(Some(&shape_vertex_function));
        shape_descriptor.setFragmentFunction(Some(&shape_fragment_function));
        let shape_attachments = shape_descriptor.colorAttachments();
        let shape_attachment = unsafe { shape_attachments.objectAtIndexedSubscript(0) };
        shape_attachment.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        shape_attachment.setBlendingEnabled(true);
        shape_attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
        shape_attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        shape_attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        shape_attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        let shape_pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&shape_descriptor)
            .map_err(|error| format!("Snow shape pipeline creation failed: {error}"))?;

        let particles = ParticleSystem::with_capacity(
            INITIAL_PARTICLE_COUNT,
            PARTICLE_CAPACITY,
            seed,
            PhysicsConfig::default(),
        );
        let buffer_length = PARTICLE_CAPACITY * std::mem::size_of::<GpuParticle>();
        let mut particle_buffers = Vec::with_capacity(GPU_BUFFER_COUNT);
        for _ in 0..GPU_BUFFER_COUNT {
            let particle_buffer = device
                .newBufferWithLength_options(buffer_length, MTLResourceOptions::StorageModeShared)
                .ok_or_else(|| "Metal failed to allocate a particle buffer".to_owned())?;
            particle_buffers.push(particle_buffer);
        }
        let mesh_buffer_length = SNOW_MESH_VERTEX_CAPACITY * std::mem::size_of::<GpuSnowVertex>();
        let mut snow_mesh_buffers = Vec::with_capacity(GPU_BUFFER_COUNT);
        for _ in 0..GPU_BUFFER_COUNT {
            let buffer = device
                .newBufferWithLength_options(
                    mesh_buffer_length,
                    MTLResourceOptions::StorageModeShared,
                )
                .ok_or_else(|| "Metal failed to allocate a snow mesh buffer".to_owned())?;
            snow_mesh_buffers.push(buffer);
        }
        let shape_buffer_length = SHAPE_VERTEX_CAPACITY * std::mem::size_of::<GpuShapeVertex>();
        let mut shape_buffers = Vec::with_capacity(GPU_BUFFER_COUNT);
        for _ in 0..GPU_BUFFER_COUNT {
            let buffer = device
                .newBufferWithLength_options(
                    shape_buffer_length,
                    MTLResourceOptions::StorageModeShared,
                )
                .ok_or_else(|| "Metal failed to allocate a snow shape buffer".to_owned())?;
            shape_buffers.push(buffer);
        }
        let mut blocked_region_buffers = Vec::with_capacity(GPU_BUFFER_COUNT);
        for _ in 0..GPU_BUFFER_COUNT {
            let buffer = device
                .newBufferWithLength_options(
                    std::mem::size_of::<GpuBlockedRegion>(),
                    MTLResourceOptions::StorageModeShared,
                )
                .ok_or_else(|| "Metal failed to allocate a window mask buffer".to_owned())?;
            blocked_region_buffers.push(buffer);
        }
        let now = Instant::now();
        Ok(Self {
            device,
            command_queue,
            particle_buffers,
            snow_mesh_buffers,
            blocked_region_buffers,
            shape_buffers,
            pipeline,
            snow_mesh_pipeline,
            shape_pipeline,
            particles,
            blocked_regions: Vec::new(),
            snow_mesh_vertices: Vec::with_capacity(SNOW_MESH_VERTEX_CAPACITY),
            shape_vertices: Vec::with_capacity(SHAPE_VERTEX_CAPACITY),
            snow_suppressed: false,
            accumulation_enabled: true,
            intensity: 1.0,
            particles_in_windows: false,
            cursor_tool: 0,
            hovered_snowball: None,
            hovered_snowman: None,
            last_cursor_position: None,
            started_at: now,
            last_update: now,
            next_buffer: 0,
        })
    }

    pub fn configure_view(&self, view: &MTKView, frame: NSRect) {
        view.setFrame(frame);
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setColorPixelFormat(MTLPixelFormat::BGRA8Unorm);
        view.setClearColor(MTLClearColor {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.0,
        });
        view.setFramebufferOnly(true);
        view.setPreferredFramesPerSecond(120);
        view.setEnableSetNeedsDisplay(false);
        view.setPaused(false);
        // MTKView inherits NSView's layer property; objc2 does not expose inherited methods here.
        let layer: Option<Retained<CALayer>> = unsafe { msg_send![view, layer] };
        if let Some(layer) = layer {
            layer.setOpaque(false);
            layer.setBackgroundColor(None);
        }
    }

    pub fn device(&self) -> &ProtocolObject<dyn MTLDevice> {
        &self.device
    }

    pub fn set_surfaces(&mut self, surfaces: &[CollisionSurface]) {
        self.particles.set_surfaces(surfaces);
    }

    pub fn set_blocked_regions(&mut self, regions: &[BlockedRegion]) -> Result<(), String> {
        self.particles.set_wind_draft_regions(regions);
        self.particles.set_snowball_obstacles(regions);
        let updated: Vec<_> = regions
            .iter()
            .map(|region| GpuBlockedRegion {
                x: region.x,
                y: region.y,
                width: region.width,
                height: region.height,
            })
            .collect();
        let required = updated
            .len()
            .max(1)
            .saturating_mul(std::mem::size_of::<GpuBlockedRegion>());
        if required > self.blocked_region_buffers[0].length() {
            let mut resized_buffers = Vec::with_capacity(GPU_BUFFER_COUNT);
            for _ in 0..GPU_BUFFER_COUNT {
                let buffer = self
                    .device
                    .newBufferWithLength_options(required, MTLResourceOptions::StorageModeShared)
                    .ok_or_else(|| "Metal failed to resize the window mask buffer".to_owned())?;
                resized_buffers.push(buffer);
            }
            self.blocked_region_buffers = resized_buffers;
        }
        self.blocked_regions = updated;
        Ok(())
    }

    pub fn set_fullscreen(&mut self, fullscreen: bool) {
        self.snow_suppressed = fullscreen;
    }

    pub fn set_settings(&mut self, settings: AppSettings) {
        let defaults = PhysicsConfig::default();
        self.particles.set_config(PhysicsConfig {
            wind_strength: settings.wind_strength,
            wind_direction_x: settings.wind_direction.cos(),
            wind_direction_y: settings.wind_direction.sin(),
            turbulence: settings.wind_turbulence,
            wind_alternates: settings.weather_mode == 2,
            minimum_terminal_speed: defaults.minimum_terminal_speed * settings.falling_speed,
            maximum_terminal_speed: defaults.maximum_terminal_speed * settings.falling_speed,
            ..defaults
        });
        self.particles
            .set_accumulation_enabled(settings.accumulation);
        self.particles
            .set_accumulation_speed(settings.accumulation_speed);
        self.particles
            .set_window_wind_draft_enabled(settings.window_wind_draft);
        self.particles
            .set_particle_count(settings.particle_count as usize);
        self.accumulation_enabled = settings.accumulation;
        self.intensity = settings.intensity;
        self.particles_in_windows = settings.particles_in_windows;
    }

    pub fn clear_snow(&mut self) {
        self.particles.clear_snow();
    }

    pub fn snowball_at(&self, x: f32, y: f32) -> Option<u64> {
        if self.snow_suppressed {
            return None;
        }
        self.particles.snowball_at(x, y)
    }

    pub fn grab_snowball(&mut self, x: f32, y: f32) -> Option<u64> {
        self.particles.grab_snowball(x, y)
    }

    pub fn drag_snowball(&mut self, id: u64, x: f32, y: f32, delta_seconds: f32) -> bool {
        self.particles.drag_snowball(id, x, y, delta_seconds)
    }

    pub fn release_snowball(&mut self, id: u64) -> bool {
        self.particles.release_snowball(id)
    }

    pub fn set_hovered_snowball(&mut self, id: Option<u64>) {
        self.hovered_snowball = id;
    }

    pub fn set_hovered_snowman(&mut self, id: Option<u64>) {
        self.hovered_snowman = id;
    }

    pub fn snowman_at(&self, x: f32, y: f32) -> Option<u64> {
        if self.snow_suppressed {
            return None;
        }
        self.particles.snowman_at(x, y)
    }

    pub fn grab_snowman(&mut self, x: f32, y: f32) -> Option<u64> {
        self.particles.grab_snowman(x, y)
    }

    pub fn drag_snowman(&mut self, id: u64, x: f32, y: f32, delta_seconds: f32) -> bool {
        self.particles.drag_snowman(id, x, y, delta_seconds)
    }

    pub fn release_snowman(&mut self, id: u64) -> bool {
        self.particles.release_snowman(id)
    }

    pub fn can_roll_snowball_at(&self, x: f32, y: f32) -> bool {
        self.cursor_tool == 30 && self.particles.can_roll_snowball_at(x, y)
    }

    pub fn roll_snowball_at(&mut self, x: f32, y: f32, direction_x: f32) -> bool {
        self.cursor_tool == 30 && self.particles.roll_snowball_at(x, y, direction_x)
    }

    pub fn cancel_overlay_drag(&mut self) {
        let held: Vec<u64> = self
            .particles
            .snowballs()
            .iter()
            .filter(|ball| ball.state == winter_physics::SnowballState::Held)
            .map(|ball| ball.id)
            .collect();
        for id in held {
            self.particles.release_snowball(id);
        }
        let held_snowmen: Vec<u64> = self
            .particles
            .snowmen()
            .iter()
            .filter(|snowman| snowman.held)
            .map(|snowman| snowman.id)
            .collect();
        for id in held_snowmen {
            self.particles.release_snowman(id);
        }
    }

    pub fn set_cursor_tool(&mut self, tool: u8) {
        if self.cursor_tool != tool {
            self.cursor_tool = tool;
            self.last_cursor_position = None;
        }
    }

    pub fn set_cursor_position(
        &mut self,
        position: Option<(f32, f32)>,
        drawable_width: f32,
        drawable_height: f32,
    ) {
        if self
            .particles
            .snowballs()
            .iter()
            .any(|ball| ball.state == winter_physics::SnowballState::Held)
            || self.particles.snowmen().iter().any(|snowman| snowman.held)
        {
            self.last_cursor_position = None;
            return;
        }
        let Some((x, y)) = position.filter(|_| self.cursor_tool != 0) else {
            self.last_cursor_position = None;
            return;
        };
        let Some((previous_x, previous_y)) = self.last_cursor_position.replace((x, y)) else {
            return;
        };
        let delta_x = x - previous_x;
        let delta_y = y - previous_y;
        if delta_x.hypot(delta_y) < 0.0002 || drawable_width <= 0.0 || drawable_height <= 0.0 {
            return;
        }

        let radius_x = (32.0 / drawable_width).max(0.001);
        let radius_y = (32.0 / drawable_height).max(0.001);
        let travel = (delta_x / radius_x).hypot(delta_y / radius_y);
        let steps = (travel / 0.25).ceil().clamp(1.0, 64.0) as usize;
        let strength = if self.cursor_tool == 31 {
            1.0
        } else {
            0.7 * (travel / steps as f32).clamp(0.05, 1.0)
        };
        let direction_length = delta_x.hypot(delta_y).max(f32::EPSILON);
        let direction_x = delta_x / direction_length;
        let direction_y = delta_y / direction_length;
        for step in 1..=steps {
            let progress = step as f32 / steps as f32;
            let brush_x = previous_x + delta_x * progress;
            let brush_y = previous_y + delta_y * progress;
            if self.cursor_tool == 31 {
                self.particles
                    .brush_snow_at(brush_x, brush_y, radius_x, radius_y, strength);
            } else {
                self.particles.push_snow_at(BrushStroke {
                    x: brush_x,
                    y: brush_y,
                    radius_x,
                    radius_y,
                    strength,
                    direction_x,
                    direction_y,
                });
            }
        }
    }

    pub fn draw(&mut self, view: &MTKView) -> bool {
        let Some(render_pass) = view.currentRenderPassDescriptor() else {
            return false;
        };
        let Some(drawable) = view.currentDrawable() else {
            return false;
        };
        let Some(command_buffer) = self.command_queue.commandBuffer() else {
            eprintln!("Metal did not provide a command buffer");
            return false;
        };
        let Some(encoder) = command_buffer.renderCommandEncoderWithDescriptor(&render_pass) else {
            eprintln!("Metal did not provide a render command encoder");
            return false;
        };

        let now = Instant::now();
        let delta_seconds = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;
        let display_size = view.drawableSize();
        if display_size.width > 0.0 && display_size.height > 0.0 {
            self.particles
                .set_display_aspect((display_size.width / display_size.height) as f32);
        }
        if !self.snow_suppressed {
            self.particles.update(delta_seconds);
        }

        let particle_buffer = &self.particle_buffers[self.next_buffer];
        let snow_mesh_buffer = &self.snow_mesh_buffers[self.next_buffer];
        let blocked_region_buffer = &self.blocked_region_buffers[self.next_buffer];
        let shape_buffer = &self.shape_buffers[self.next_buffer];
        // MTKView limits drawable frames in flight; this triple buffer keeps CPU writes
        // separate from the GPU reads of the two preceding submitted frames.
        let (draw_count, snow_mesh_vertex_count, shape_vertex_count, uniforms);
        unsafe {
            let output = particle_buffer.contents().cast::<GpuParticle>().as_ptr();
            let particle_count = self.particles.particles().len();
            let visible_particle_count = (particle_count as f32 * self.intensity).round() as usize;
            for (index, particle) in self
                .particles
                .particles()
                .iter()
                .take(visible_particle_count.min(PARTICLE_CAPACITY))
                .enumerate()
            {
                output.add(index).write(GpuParticle::from(particle));
            }
            let drawable_size = view.drawableSize();
            self.snow_mesh_vertices.clear();
            if self.accumulation_enabled && drawable_size.width > 0.0 && drawable_size.height > 0.0
            {
                for surface in self.particles.snow_surfaces() {
                    append_snow_mesh(&mut self.snow_mesh_vertices, surface);
                }
            }
            if self.snow_mesh_vertices.len() > SNOW_MESH_VERTEX_CAPACITY {
                eprintln!(
                    "Snow mesh exceeded its GPU buffer capacity: {} vertices",
                    self.snow_mesh_vertices.len()
                );
                self.snow_mesh_vertices.truncate(SNOW_MESH_VERTEX_CAPACITY);
            }
            snow_mesh_vertex_count = self.snow_mesh_vertices.len();
            let mesh_output = snow_mesh_buffer.contents().cast::<GpuSnowVertex>().as_ptr();
            for (index, vertex) in self.snow_mesh_vertices.iter().copied().enumerate() {
                mesh_output.add(index).write(vertex);
            }
            self.shape_vertices.clear();
            if !self.snow_suppressed && drawable_size.width > 0.0 && drawable_size.height > 0.0 {
                let aspect = (drawable_size.width / drawable_size.height) as f32;
                for ball in self.particles.snowballs() {
                    if self.hovered_snowball == Some(ball.id) {
                        append_circle(
                            &mut self.shape_vertices,
                            ball.x,
                            ball.y,
                            ball.radius * 1.55,
                            aspect,
                            (0.48, 0.68, 1.0, 0.30),
                        );
                    }
                    append_snowball(&mut self.shape_vertices, ball, aspect);
                }
                for snowman in self.particles.snowmen() {
                    if self.hovered_snowman == Some(snowman.id) {
                        append_circle(
                            &mut self.shape_vertices,
                            snowman.x,
                            snowman.y - snowman.radius,
                            snowman.radius * 1.6,
                            aspect,
                            (0.48, 0.68, 1.0, 0.26),
                        );
                    }
                    append_snowman(&mut self.shape_vertices, snowman, aspect);
                }
            }
            if self.shape_vertices.len() > SHAPE_VERTEX_CAPACITY {
                eprintln!(
                    "Snow shapes exceeded their GPU buffer capacity: {} vertices",
                    self.shape_vertices.len()
                );
                self.shape_vertices.truncate(SHAPE_VERTEX_CAPACITY);
            }
            shape_vertex_count = self.shape_vertices.len();
            let shape_output = shape_buffer.contents().cast::<GpuShapeVertex>().as_ptr();
            for (index, vertex) in self.shape_vertices.iter().copied().enumerate() {
                shape_output.add(index).write(vertex);
            }
            if !self.blocked_regions.is_empty() {
                let blocked_output = blocked_region_buffer
                    .contents()
                    .cast::<GpuBlockedRegion>()
                    .as_ptr();
                for (index, region) in self.blocked_regions.iter().copied().enumerate() {
                    blocked_output.add(index).write(region);
                }
            }
            uniforms = FrameUniforms {
                elapsed_time: self.started_at.elapsed().as_secs_f32(),
                blocked_region_count: self.blocked_regions.len() as u32,
                drawable_width: drawable_size.width as f32,
                drawable_height: drawable_size.height as f32,
                snow_suppressed: u32::from(self.snow_suppressed),
                particles_in_windows: u32::from(self.particles_in_windows),
            };
            draw_count = visible_particle_count.min(PARTICLE_CAPACITY);
            self.next_buffer = (self.next_buffer + 1) % GPU_BUFFER_COUNT;
            encoder.setRenderPipelineState(&self.pipeline);
            encoder.setVertexBuffer_offset_atIndex(Some(particle_buffer), 0, 0);
            encoder.setVertexBytes_length_atIndex(
                std::ptr::NonNull::from(&uniforms).cast(),
                std::mem::size_of::<FrameUniforms>(),
                1,
            );
            encoder.setFragmentBytes_length_atIndex(
                std::ptr::NonNull::from(&uniforms).cast(),
                std::mem::size_of::<FrameUniforms>(),
                1,
            );
            encoder.setFragmentBuffer_offset_atIndex(Some(blocked_region_buffer), 0, 2);
            if draw_count > 0 {
                encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                    MTLPrimitiveType::Point,
                    0,
                    draw_count,
                    1,
                );
            }
            if snow_mesh_vertex_count > 0 {
                encoder.setRenderPipelineState(&self.snow_mesh_pipeline);
                encoder.setVertexBuffer_offset_atIndex(Some(snow_mesh_buffer), 0, 0);
                encoder.setVertexBytes_length_atIndex(
                    std::ptr::NonNull::from(&uniforms).cast(),
                    std::mem::size_of::<FrameUniforms>(),
                    1,
                );
                encoder.setFragmentBytes_length_atIndex(
                    std::ptr::NonNull::from(&uniforms).cast(),
                    std::mem::size_of::<FrameUniforms>(),
                    1,
                );
                encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                    MTLPrimitiveType::Triangle,
                    0,
                    snow_mesh_vertex_count,
                    1,
                );
            }
            if shape_vertex_count > 0 {
                encoder.setRenderPipelineState(&self.shape_pipeline);
                encoder.setVertexBuffer_offset_atIndex(Some(shape_buffer), 0, 0);
                encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                    MTLPrimitiveType::Triangle,
                    0,
                    shape_vertex_count,
                    1,
                );
            }
        }

        encoder.endEncoding();
        command_buffer.presentDrawable(ProtocolObject::from_ref(&*drawable));
        command_buffer.commit();
        true
    }
}

fn append_snow_mesh(vertices: &mut Vec<GpuSnowVertex>, surface: &winter_physics::SnowSurface) {
    use winter_physics::SurfaceOrientation;

    if surface.snow_depths.iter().all(|depth| *depth <= 0.00001) {
        return;
    }
    if !surface.edge_margin.is_finite()
        || surface.edge_margin < 0.0
        || surface.edge_margin * 2.0 >= surface.width
    {
        return;
    }

    let profile_start = surface.edge_margin / surface.width;
    let snow_x = surface.x + surface.edge_margin;
    let snow_width = surface.width - surface.edge_margin * 2.0;
    let sample_count = ACCUMULATION_BINS * ACCUMULATION_SUBDIVISIONS;
    let mut previous_depth = 0.0;
    for sample in 0..sample_count {
        let start = sample as f32 / sample_count as f32;
        let end = (sample + 1) as f32 / sample_count as f32;
        let depth_at = |progress: f32| {
            let profile_progress = profile_start + progress * (1.0 - profile_start * 2.0);
            let profile = profile_progress * ACCUMULATION_BINS as f32;
            let bin = (profile.floor() as usize).min(ACCUMULATION_BINS - 1);
            let t = (profile - bin as f32).clamp(0.0, 1.0);
            let depth = smooth_depth(&surface.snow_depths, bin, t);
            let fade = (progress / 0.025)
                .clamp(0.0, 1.0)
                .min(((1.0 - progress) / 0.025).clamp(0.0, 1.0));
            depth * fade * fade * (3.0 - 2.0 * fade)
        };
        let start_depth = if sample == 0 { 0.0 } else { previous_depth };
        let end_depth = depth_at(end);
        previous_depth = end_depth;

        match surface.orientation {
            SurfaceOrientation::Horizontal => {
                let x0 = snow_x + snow_width * start;
                let x1 = snow_x + snow_width * end;
                let top0 = surface.y - start_depth;
                let top1 = surface.y - end_depth;
                let base = surface.y;
                vertices.extend_from_slice(&[
                    GpuSnowVertex { x: x0, y: top0 },
                    GpuSnowVertex { x: x0, y: base },
                    GpuSnowVertex { x: x1, y: base },
                    GpuSnowVertex { x: x0, y: top0 },
                    GpuSnowVertex { x: x1, y: base },
                    GpuSnowVertex { x: x1, y: top1 },
                ]);
            }
            SurfaceOrientation::VerticalToRight | SurfaceOrientation::VerticalToLeft => {
                let y0 = surface.y + surface.edge_margin + snow_width * start;
                let y1 = surface.y + surface.edge_margin + snow_width * end;
                let outward = if surface.orientation == SurfaceOrientation::VerticalToRight {
                    1.0
                } else {
                    -1.0
                };
                let front0 = surface.x + outward * start_depth;
                let front1 = surface.x + outward * end_depth;
                vertices.extend_from_slice(&[
                    GpuSnowVertex {
                        x: surface.x,
                        y: y0,
                    },
                    GpuSnowVertex { x: front0, y: y0 },
                    GpuSnowVertex { x: front1, y: y1 },
                    GpuSnowVertex {
                        x: surface.x,
                        y: y0,
                    },
                    GpuSnowVertex { x: front1, y: y1 },
                    GpuSnowVertex {
                        x: surface.x,
                        y: y1,
                    },
                ]);
            }
        }
    }
}

fn smooth_depth(depths: &[f32], bin: usize, t: f32) -> f32 {
    let last = depths.len().saturating_sub(1);
    let at = |offset: isize| depths[(bin as isize + offset).clamp(0, last as isize) as usize];
    let p0 = at(-1);
    let p1 = at(0);
    let p2 = at(1);
    let p3 = at(2);
    let t2 = t * t;
    let t3 = t2 * t;
    (0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3))
        .clamp(p1.min(p2), p1.max(p2))
}

type ShapeColor = (f32, f32, f32, f32);

fn push_shape_vertex(vertices: &mut Vec<GpuShapeVertex>, x: f32, y: f32, color: ShapeColor) {
    vertices.push(GpuShapeVertex {
        x,
        y,
        _padding: [0.0; 2],
        red: color.0,
        green: color.1,
        blue: color.2,
        alpha: color.3,
    });
}

fn push_triangle(
    vertices: &mut Vec<GpuShapeVertex>,
    first: (f32, f32),
    second: (f32, f32),
    third: (f32, f32),
    color: ShapeColor,
) {
    push_shape_vertex(vertices, first.0, first.1, color);
    push_shape_vertex(vertices, second.0, second.1, color);
    push_shape_vertex(vertices, third.0, third.1, color);
}

fn push_quad(vertices: &mut Vec<GpuShapeVertex>, corners: [(f32, f32); 4], color: ShapeColor) {
    push_triangle(vertices, corners[0], corners[1], corners[2], color);
    push_triangle(vertices, corners[0], corners[2], corners[3], color);
}

fn append_circle(
    vertices: &mut Vec<GpuShapeVertex>,
    center_x: f32,
    center_y: f32,
    radius_y: f32,
    aspect: f32,
    color: ShapeColor,
) {
    if radius_y <= 0.0 || !aspect.is_finite() || aspect <= 0.0 {
        return;
    }
    let radius_x = radius_y / aspect;
    let segments = SHAPE_CIRCLE_SEGMENTS;
    for index in 0..segments {
        let start = index as f32 / segments as f32 * std::f32::consts::TAU;
        let end = (index + 1) as f32 / segments as f32 * std::f32::consts::TAU;
        push_triangle(
            vertices,
            (center_x, center_y),
            (
                center_x + start.cos() * radius_x,
                center_y + start.sin() * radius_y,
            ),
            (
                center_x + end.cos() * radius_x,
                center_y + end.sin() * radius_y,
            ),
            color,
        );
    }
}

fn append_thick_line(
    vertices: &mut Vec<GpuShapeVertex>,
    start: (f32, f32),
    end: (f32, f32),
    thickness: f32,
    aspect: f32,
    color: ShapeColor,
) {
    let delta_x = (end.0 - start.0) * aspect;
    let delta_y = end.1 - start.1;
    let length = delta_x.hypot(delta_y).max(f32::EPSILON);
    let normal_x = -delta_y / length * thickness * 0.5 / aspect;
    let normal_y = delta_x / length * thickness * 0.5;
    push_quad(
        vertices,
        [
            (start.0 + normal_x, start.1 + normal_y),
            (end.0 + normal_x, end.1 + normal_y),
            (end.0 - normal_x, end.1 - normal_y),
            (start.0 - normal_x, start.1 - normal_y),
        ],
        color,
    );
}

fn append_snowball(vertices: &mut Vec<GpuShapeVertex>, ball: &Snowball, aspect: f32) {
    let radius = ball.radius;
    append_circle(
        vertices,
        ball.x,
        ball.y,
        radius * 1.06,
        aspect,
        (0.52, 0.66, 0.92, 0.40),
    );
    append_circle(
        vertices,
        ball.x,
        ball.y,
        radius,
        aspect,
        (0.93, 0.97, 1.0, 0.98),
    );
    append_circle(
        vertices,
        ball.x - radius * 0.30 / aspect,
        ball.y - radius * 0.34,
        radius * 0.38,
        aspect,
        (1.0, 1.0, 1.0, 0.62),
    );
    append_circle(
        vertices,
        ball.x + radius * 0.18 / aspect,
        ball.y + radius * 0.45,
        radius * 0.55,
        aspect,
        (0.78, 0.87, 0.99, 0.22),
    );
}

fn append_snowman(vertices: &mut Vec<GpuShapeVertex>, snowman: &Snowman, aspect: f32) {
    let radius = snowman.radius;
    if radius <= 0.0 || !aspect.is_finite() || aspect <= 0.0 {
        return;
    }
    let middle_radius = radius * 0.78;
    let head_radius = radius * 0.62;
    let bottom_y = snowman.y - radius;
    let middle_y = bottom_y - (radius + middle_radius) * 0.92;
    let head_y = middle_y - (middle_radius + head_radius) * 0.92;
    let snow = (0.93, 0.97, 1.0, 0.98);
    let outline = (0.60, 0.72, 0.95, 0.42);

    // Twig arms behind the body.
    let arm_color = (0.34, 0.21, 0.11, 1.0);
    for direction in [-1.0_f32, 1.0] {
        let start = (
            snowman.x + direction * middle_radius * 0.55 / aspect,
            middle_y - middle_radius * 0.05,
        );
        let end = (
            snowman.x + direction * middle_radius * 1.75 / aspect,
            middle_y - middle_radius * 0.80,
        );
        append_thick_line(vertices, start, end, radius * 0.10, aspect, arm_color);
        let twig = (
            end.0 + direction * middle_radius * 0.38 / aspect,
            end.1 + middle_radius * 0.30,
        );
        append_thick_line(vertices, end, twig, radius * 0.075, aspect, arm_color);
    }

    for (center_y, body_radius) in [
        (bottom_y, radius),
        (middle_y, middle_radius),
        (head_y, head_radius),
    ] {
        append_circle(
            vertices,
            snowman.x,
            center_y,
            body_radius * 1.04,
            aspect,
            outline,
        );
        append_circle(vertices, snowman.x, center_y, body_radius, aspect, snow);
    }
    // Soft highlights on the three body sections.
    for (center_y, body_radius) in [
        (bottom_y, radius),
        (middle_y, middle_radius),
        (head_y, head_radius),
    ] {
        append_circle(
            vertices,
            snowman.x - body_radius * 0.28 / aspect,
            center_y - body_radius * 0.30,
            body_radius * 0.34,
            aspect,
            (1.0, 1.0, 1.0, 0.55),
        );
    }

    // Bucket hat: brim plus a tapered crown.
    let bucket_dark = (0.24, 0.29, 0.39, 1.0);
    let bucket_light = (0.38, 0.44, 0.57, 1.0);
    let brim_y = head_y - head_radius * 0.68;
    let brim_half_width = head_radius * 0.86 / aspect;
    let brim_height = head_radius * 0.19;
    push_quad(
        vertices,
        [
            (snowman.x - brim_half_width, brim_y),
            (snowman.x + brim_half_width, brim_y),
            (snowman.x + brim_half_width, brim_y - brim_height),
            (snowman.x - brim_half_width, brim_y - brim_height),
        ],
        bucket_light,
    );
    let crown_bottom = brim_y - brim_height;
    let crown_top = crown_bottom - head_radius * 0.86;
    let bottom_half = head_radius * 0.74 / aspect;
    let top_half = head_radius * 0.58 / aspect;
    push_quad(
        vertices,
        [
            (snowman.x - bottom_half, crown_bottom),
            (snowman.x + bottom_half, crown_bottom),
            (snowman.x + top_half, crown_top),
            (snowman.x - top_half, crown_top),
        ],
        bucket_dark,
    );
    push_quad(
        vertices,
        [
            (snowman.x - bottom_half * 0.92, crown_bottom),
            (snowman.x - bottom_half * 0.55, crown_bottom),
            (snowman.x - top_half * 0.55, crown_top),
            (snowman.x - top_half * 0.90, crown_top),
        ],
        (0.38, 0.44, 0.57, 0.75),
    );

    // Carrot nose, coal eyes and two red buttons.
    let nose_color = (1.0, 0.53, 0.10, 1.0);
    push_triangle(
        vertices,
        (
            snowman.x + head_radius * 0.32 / aspect,
            head_y - head_radius * 0.04,
        ),
        (
            snowman.x + head_radius * 1.38 / aspect,
            head_y + head_radius * 0.07,
        ),
        (
            snowman.x + head_radius * 0.32 / aspect,
            head_y + head_radius * 0.22,
        ),
        nose_color,
    );
    let coal = (0.05, 0.06, 0.09, 1.0);
    for direction in [-1.0_f32, 1.0] {
        append_circle(
            vertices,
            snowman.x + direction * head_radius * 0.27 / aspect,
            head_y - head_radius * 0.22,
            head_radius * 0.095,
            aspect,
            coal,
        );
    }
    let button = (0.72, 0.09, 0.13, 1.0);
    for offset in [-0.22_f32, 0.30] {
        append_circle(
            vertices,
            snowman.x,
            middle_y + middle_radius * offset,
            middle_radius * 0.10,
            aspect,
            button,
        );
    }
}

impl From<&Particle> for GpuParticle {
    fn from(particle: &Particle) -> Self {
        Self {
            x: particle.x,
            y: particle.y,
            diameter: particle.diameter,
            opacity: particle.opacity,
            phase: particle.phase,
            rotation: particle.rotation,
            depth: particle.depth,
            window_draft: if particle.window_draft { 1.0 } else { 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_snow_mesh, append_snowball, append_snowman, GpuSnowVertex, ACCUMULATION_BINS,
        ACCUMULATION_SUBDIVISIONS,
    };
    use winter_physics::{SnowSurface, Snowball, SnowballState, Snowman, NO_SURFACE_ID};

    #[test]
    fn snow_accumulation_mesh_is_continuous_and_respects_edge_margins() {
        let surface = SnowSurface {
            id: 1,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.02,
            orientation: winter_physics::SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
            snow_depths: vec![0.012; ACCUMULATION_BINS],
        };
        let mut vertices = Vec::<GpuSnowVertex>::new();

        append_snow_mesh(&mut vertices, &surface);

        let expected_vertex_count = ACCUMULATION_BINS * ACCUMULATION_SUBDIVISIONS * 6;
        assert_eq!(vertices.len(), expected_vertex_count);
        assert!((vertices[0].x - (surface.x + surface.edge_margin)).abs() < f32::EPSILON);
        let last = vertices.last().unwrap();
        assert!((last.x - (surface.x + surface.width - surface.edge_margin)).abs() < f32::EPSILON);
        for quad in vertices.chunks_exact(6) {
            assert!(quad.iter().all(|vertex| vertex.y <= surface.y));
        }
        for adjacent_quads in vertices.chunks_exact(6).collect::<Vec<_>>().windows(2) {
            let previous_end = adjacent_quads[0][5];
            let next_start = adjacent_quads[1][0];
            assert!((previous_end.x - next_start.x).abs() < f32::EPSILON);
            assert!((previous_end.y - next_start.y).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn empty_accumulation_does_not_generate_mesh_geometry() {
        let surface = SnowSurface {
            id: 1,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.02,
            orientation: winter_physics::SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
            snow_depths: vec![0.0; ACCUMULATION_BINS],
        };
        let mut vertices = Vec::new();

        append_snow_mesh(&mut vertices, &surface);

        assert!(vertices.is_empty());
    }

    #[test]
    fn vertical_dock_surfaces_render_snow_outward_from_the_dock_face() {
        let surface = SnowSurface {
            id: 2,
            x: 0.4,
            y: 0.2,
            width: 0.6,
            edge_margin: 0.0,
            orientation: winter_physics::SurfaceOrientation::VerticalToRight,
            persistent: true,
            maximum_depth: 0.03,
            snow_depths: vec![0.01; ACCUMULATION_BINS],
        };
        let mut vertices = Vec::new();

        append_snow_mesh(&mut vertices, &surface);

        let expected_vertex_count = ACCUMULATION_BINS * ACCUMULATION_SUBDIVISIONS * 6;
        assert_eq!(vertices.len(), expected_vertex_count);
        assert!(vertices.iter().all(|vertex| vertex.x >= surface.x));
        assert!(vertices.iter().all(|vertex| vertex.x <= surface.x + 0.01));
        assert!((vertices[0].y - surface.y).abs() < f32::EPSILON);
        assert!((vertices.last().unwrap().y - (surface.y + surface.width)).abs() < f32::EPSILON);
    }

    #[test]
    fn snowball_geometry_is_round_and_stays_inside_the_ball() {
        let ball = Snowball {
            id: 1,
            x: 0.5,
            y: 0.5,
            velocity_x: 0.0,
            velocity_y: 0.0,
            radius: 0.02,
            state: SnowballState::Resting,
            group: 0,
            surface_id: NO_SURFACE_ID,
            surface_offset_x: 0.0,
            roll: 0.0,
            placed: false,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
        };
        let mut vertices = Vec::new();

        append_snowball(&mut vertices, &ball, 2.0);

        assert!(!vertices.is_empty());
        for vertex in &vertices {
            let delta_x = (vertex.x - ball.x) * 2.0;
            let delta_y = vertex.y - ball.y;
            assert!(delta_x.hypot(delta_y) <= ball.radius * 1.1);
        }
        assert!(vertices.iter().any(|vertex| vertex.alpha > 0.9));
    }

    #[test]
    fn snowman_geometry_has_bucket_nose_eyes_and_buttons() {
        let snowman = Snowman {
            id: 1,
            x: 0.5,
            y: 0.8,
            radius: 0.02,
            surface_id: NO_SURFACE_ID,
            surface_offset_x: 0.0,
            held: false,
            velocity_x: 0.0,
            velocity_y: 0.0,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
            drag_offset_x: 0.0,
            drag_offset_y: 0.0,
        };
        let mut vertices = Vec::new();

        append_snowman(&mut vertices, &snowman, 2.0);

        assert!(vertices.len() > 300);
        assert!(vertices.iter().any(|vertex| {
            vertex.red > 0.9 && (0.4..0.65).contains(&vertex.green) && vertex.blue < 0.25
        }));
        assert!(vertices
            .iter()
            .any(|vertex| { vertex.red > 0.6 && vertex.green < 0.2 && vertex.blue < 0.2 }));
        assert!(vertices
            .iter()
            .any(|vertex| { vertex.red < 0.4 && vertex.green < 0.4 && vertex.blue < 0.6 }));
        let top = vertices
            .iter()
            .map(|vertex| vertex.y)
            .fold(1.0_f32, f32::min);
        let bottom = vertices
            .iter()
            .map(|vertex| vertex.y)
            .fold(0.0_f32, f32::max);
        assert!(top < snowman.y - snowman.radius * 3.5);
        assert!(bottom >= snowman.y - 0.0001);
    }
}
