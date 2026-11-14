//! Shared snowfall model: this module owns particle state, external forces,
//! collision surfaces, and accumulated heightfields. Positions and dimensions
//! are normalized to the display with a top-left origin; GPU pixel sizes stay
//! outside the simulation.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsConfig {
    pub gravity: f32,
    pub minimum_terminal_speed: f32,
    pub maximum_terminal_speed: f32,
    pub wind_strength: f32,
    pub wind_direction_x: f32,
    pub wind_direction_y: f32,
    pub turbulence: f32,
    /// When true the wind swings through both directions (blizzard). Otherwise
    /// it keeps the selected direction and only varies in strength.
    pub wind_alternates: bool,
}

#[test]
fn recycled_particles_spawn_at_the_top_display_boundary() {
    let mut system = ParticleSystem::new(1, 37, PhysicsConfig::default());
    system.particles[0].y = 1.03;

    system.update(1.0 / 120.0);

    assert_eq!(system.particles()[0].y, 0.0);
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: 0.045,
            minimum_terminal_speed: 0.035,
            maximum_terminal_speed: 0.105,
            wind_strength: 0.018,
            wind_direction_x: 1.0,
            wind_direction_y: 0.0,
            turbulence: 0.014,
            wind_alternates: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Particle position and velocity use normalized display coordinates (0..1).
pub struct Particle {
    pub x: f32,
    pub y: f32,
    pub velocity_x: f32,
    pub velocity_y: f32,
    pub terminal_speed: f32,
    pub diameter: f32,
    pub rotation: f32,
    pub rotation_speed: f32,
    pub opacity: f32,
    pub depth: f32,
    pub phase: f32,
    pub wind_influence: f32,
    pub window_draft: bool,
    pub window_draft_entered: bool,
    pub draft_window_id: u64,
    pub draft_velocity_x: f32,
    pub draft_velocity_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// A normalized top-left-origin display rectangle used for clipping and drafts.
pub struct BlockedRegion {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnowballState {
    Free,
    Held,
    Resting,
}

/// A hand-rolled snowball. Positions and the radius live in normalized display
/// units with a top-left origin, exactly like particle and surface geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snowball {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub velocity_x: f32,
    pub velocity_y: f32,
    pub radius: f32,
    pub state: SnowballState,
    /// Snowballs that touch share a group id (the smallest id in the group).
    /// Three connected snowballs turn into a snowman.
    pub group: u64,
    pub surface_id: u64,
    /// Horizontal offset from the left edge of `surface_id`.
    pub surface_offset_x: f32,
    pub roll: f32,
    /// Set when the user drops a held ball. Only such deliberate drops may
    /// stick to another ball; rolling balls merely bump into each other.
    pub placed: bool,
    pub drag_velocity_x: f32,
    pub drag_velocity_y: f32,
}

/// A three-snowball creation. `x`/`y` are the center and the bottom of the
/// snowman; `radius` is the radius of its bottom ball.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snowman {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub surface_id: u64,
    pub surface_offset_x: f32,
    pub held: bool,
    pub velocity_x: f32,
    pub velocity_y: f32,
    pub drag_velocity_x: f32,
    pub drag_velocity_y: f32,
    pub drag_offset_x: f32,
    pub drag_offset_y: f32,
}

pub const NO_SURFACE_ID: u64 = u64::MAX;

#[derive(Debug, Clone, Copy)]
struct DraftWindow {
    id: u64,
    region: BlockedRegion,
    next_gust_at: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushStroke {
    pub x: f32,
    pub y: f32,
    pub radius_x: f32,
    pub radius_y: f32,
    pub strength: f32,
    pub direction_x: f32,
    pub direction_y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceOrientation {
    Horizontal,
    VerticalToRight,
    VerticalToLeft,
}

#[derive(Debug)]
pub struct ParticleSystem {
    config: PhysicsConfig,
    particles: Vec<Particle>,
    elapsed: f32,
    random_state: u32,
    snow_surfaces: Vec<SnowSurface>,
    accumulation_enabled: bool,
    accumulation_speed: f32,
    window_wind_draft_enabled: bool,
    draft_windows: Vec<DraftWindow>,
    display_aspect: f32,
    snowballs: Vec<Snowball>,
    snowmen: Vec<Snowman>,
    snowball_obstacles: Vec<BlockedRegion>,
    next_snowball_id: u64,
    next_snowman_id: u64,
    next_snowball_spawn_at: f32,
}

fn snowman_collision_capsule(snowman: &Snowman) -> (f32, f32, f32, f32) {
    let radius = snowman.radius;
    (
        snowman.x,
        snowman.y - radius * 4.25,
        snowman.y - radius * 0.85,
        radius * 0.85,
    )
}

fn push_out_of_snowman(
    point_x: &mut f32,
    point_y: &mut f32,
    ball_radius: f32,
    snowman: &Snowman,
    aspect: f32,
) -> bool {
    let (center_x, top, bottom, capsule_radius) = snowman_collision_capsule(snowman);
    let nearest_y = point_y.clamp(top, bottom);
    let delta_x = (*point_x - center_x) * aspect;
    let delta_y = *point_y - nearest_y;
    let minimum = ball_radius + capsule_radius;
    let distance_squared = delta_x * delta_x + delta_y * delta_y;
    if distance_squared >= minimum * minimum {
        return false;
    }
    let distance = distance_squared.sqrt();
    let (normal_x, normal_y) = if distance > f32::EPSILON {
        (delta_x / distance, delta_y / distance)
    } else {
        (1.0, 0.0)
    };
    let overlap = minimum - distance;
    *point_x += normal_x * overlap / aspect.max(f32::EPSILON);
    *point_y += normal_y * overlap;
    true
}

fn push_out_of_circle(
    point_x: &mut f32,
    point_y: &mut f32,
    radius: f32,
    other_x: f32,
    other_y: f32,
    other_radius: f32,
    aspect: f32,
) -> bool {
    let delta_x = (*point_x - other_x) * aspect;
    let delta_y = *point_y - other_y;
    let minimum = radius + other_radius;
    let distance_squared = delta_x * delta_x + delta_y * delta_y;
    if distance_squared >= minimum * minimum {
        return false;
    }
    let distance = distance_squared.sqrt();
    let (normal_x, normal_y) = if distance > f32::EPSILON {
        (delta_x / distance, delta_y / distance)
    } else {
        (1.0, 0.0)
    };
    let overlap = minimum - distance;
    *point_x += normal_x * overlap / aspect.max(f32::EPSILON);
    *point_y += normal_y * overlap;
    true
}

impl ParticleSystem {
    pub fn new(count: usize, seed: u32, config: PhysicsConfig) -> Self {
        Self::with_capacity(count, count, seed, config)
    }

    pub fn with_capacity(count: usize, capacity: usize, seed: u32, config: PhysicsConfig) -> Self {
        let mut system = Self {
            config: validate_config(config),
            particles: Vec::with_capacity(count.max(capacity)),
            elapsed: 0.0,
            random_state: seed.max(1),
            snow_surfaces: Vec::new(),
            accumulation_enabled: true,
            accumulation_speed: 1.0,
            window_wind_draft_enabled: false,
            draft_windows: Vec::new(),
            display_aspect: DEFAULT_DISPLAY_ASPECT,
            snowballs: Vec::new(),
            snowmen: Vec::new(),
            snowball_obstacles: Vec::new(),
            next_snowball_id: 1,
            next_snowman_id: 1,
            next_snowball_spawn_at: 0.0,
        };
        for _ in 0..count {
            let particle = system.spawn(true);
            system.particles.push(particle);
        }
        system
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn config(&self) -> PhysicsConfig {
        self.config
    }

    pub fn snow_surfaces(&self) -> &[SnowSurface] {
        &self.snow_surfaces
    }

    pub fn snowballs(&self) -> &[Snowball] {
        &self.snowballs
    }

    pub fn snowmen(&self) -> &[Snowman] {
        &self.snowmen
    }

    /// Window rectangles that snowballs bounce off and break against.
    pub fn set_snowball_obstacles(&mut self, regions: &[BlockedRegion]) {
        self.snowball_obstacles.clear();
        for region in regions.iter().copied().take(MAX_SNOWBALL_OBSTACLES) {
            if !region.x.is_finite()
                || !region.y.is_finite()
                || !region.width.is_finite()
                || !region.height.is_finite()
                || region.width <= 0.0
                || region.height <= 0.0
                || region.x < 0.0
                || region.y < 0.0
                || region.x + region.width > 1.001
                || region.y + region.height > 1.001
                || self
                    .snowball_obstacles
                    .iter()
                    .any(|existing| existing.id == region.id)
            {
                continue;
            }
            self.snowball_obstacles.push(region);
        }
    }

    /// Finds the topmost snowball under a normalized display point. The hit
    /// radius is slightly larger than the drawn ball so it stays easy to grab.
    pub fn snowball_at(&self, x: f32, y: f32) -> Option<u64> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let aspect = self.display_aspect;
        self.snowballs
            .iter()
            .rev()
            .find(|ball| {
                let delta_x = (ball.x - x) * aspect;
                let delta_y = ball.y - y;
                let radius = ball.radius * SNOWBALL_HIT_RADIUS_SCALE;
                delta_x * delta_x + delta_y * delta_y <= radius * radius
            })
            .map(|ball| ball.id)
    }

    pub fn grab_snowball(&mut self, x: f32, y: f32) -> Option<u64> {
        let id = self.snowball_at(x, y)?;
        for ball in &mut self.snowballs {
            if ball.state == SnowballState::Held {
                ball.state = SnowballState::Free;
            }
        }
        let index = self.snowballs.iter().position(|ball| ball.id == id)?;
        self.detach_group_at(index);
        let ball = &mut self.snowballs[index];
        ball.state = SnowballState::Held;
        ball.surface_id = NO_SURFACE_ID;
        ball.placed = false;
        ball.drag_velocity_x = 0.0;
        ball.drag_velocity_y = 0.0;
        Some(id)
    }

    pub fn drag_snowball(&mut self, id: u64, x: f32, y: f32, delta_seconds: f32) -> bool {
        let Some(index) = self.snowballs.iter().position(|ball| ball.id == id) else {
            return false;
        };
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let radius = self.snowballs[index].radius;
        let aspect = self.display_aspect;
        let radius_x = radius / aspect;
        let mut x = x.clamp(radius_x, 1.0 - radius_x);
        let mut y = y.clamp(radius, 1.0 - radius);
        let obstacles: Vec<(f32, f32, f32)> = self
            .snowballs
            .iter()
            .enumerate()
            .filter(|(other_index, _)| *other_index != index)
            .map(|(_, ball)| (ball.x, ball.y, ball.radius))
            .collect();
        let snowmen: Vec<Snowman> = self.snowmen.to_vec();
        for _ in 0..3 {
            for (other_x, other_y, other_radius) in &obstacles {
                push_out_of_circle(
                    &mut x,
                    &mut y,
                    radius,
                    *other_x,
                    *other_y,
                    *other_radius,
                    aspect,
                );
            }
            for snowman in &snowmen {
                push_out_of_snowman(&mut x, &mut y, radius, snowman, aspect);
            }
            x = x.clamp(radius_x, 1.0 - radius_x);
            y = y.clamp(radius, 1.0 - radius);
        }
        let dt = delta_seconds.clamp(1.0 / 240.0, 0.05);
        let ball = &mut self.snowballs[index];
        ball.drag_velocity_x = ((x - ball.x) / dt).clamp(-2.0, 2.0);
        ball.drag_velocity_y = ((y - ball.y) / dt).clamp(-2.0, 2.0);
        ball.x = x;
        ball.y = y;
        ball.velocity_x = 0.0;
        ball.velocity_y = 0.0;
        ball.state = SnowballState::Held;
        true
    }

    pub fn release_snowball(&mut self, id: u64) -> bool {
        let Some(index) = self.snowballs.iter().position(|ball| ball.id == id) else {
            return false;
        };
        let ball = &mut self.snowballs[index];
        ball.state = SnowballState::Free;
        ball.placed = true;
        ball.velocity_x = ball.drag_velocity_x.clamp(-0.8, 0.8);
        ball.velocity_y = ball.drag_velocity_y.clamp(-0.8, 0.8);
        ball.drag_velocity_x = 0.0;
        ball.drag_velocity_y = 0.0;
        true
    }

    pub fn snowman_at(&self, x: f32, y: f32) -> Option<u64> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let aspect = self.display_aspect;
        self.snowmen
            .iter()
            .rev()
            .find(|snowman| {
                let half_width = snowman.radius * 1.6 / aspect;
                x >= snowman.x - half_width
                    && x <= snowman.x + half_width
                    && y >= snowman.y - snowman.radius * 4.7
                    && y <= snowman.y + snowman.radius * 0.15
            })
            .map(|snowman| snowman.id)
    }

    pub fn grab_snowman(&mut self, x: f32, y: f32) -> Option<u64> {
        let id = self.snowman_at(x, y)?;
        for snowball in &mut self.snowballs {
            if snowball.state == SnowballState::Held {
                snowball.state = SnowballState::Free;
            }
        }
        for snowman in &mut self.snowmen {
            snowman.held = false;
        }
        let index = self.snowmen.iter().position(|snowman| snowman.id == id)?;
        let snowman = &mut self.snowmen[index];
        snowman.held = true;
        snowman.surface_id = NO_SURFACE_ID;
        snowman.surface_offset_x = 0.0;
        snowman.velocity_x = 0.0;
        snowman.velocity_y = 0.0;
        snowman.drag_velocity_x = 0.0;
        snowman.drag_velocity_y = 0.0;
        snowman.drag_offset_x = x - snowman.x;
        snowman.drag_offset_y = y - snowman.y;
        Some(id)
    }

    pub fn drag_snowman(&mut self, id: u64, x: f32, y: f32, delta_seconds: f32) -> bool {
        let Some(index) = self.snowmen.iter().position(|snowman| snowman.id == id) else {
            return false;
        };
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let radius = self.snowmen[index].radius;
        let radius_x = radius / self.display_aspect;
        let height = radius * 4.7;
        let target_x = (x - self.snowmen[index].drag_offset_x).clamp(radius_x, 1.0 - radius_x);
        let target_y = (y - self.snowmen[index].drag_offset_y).clamp(height, 1.0);
        let dt = delta_seconds.clamp(1.0 / 240.0, 0.05);
        let snowman = &mut self.snowmen[index];
        snowman.drag_velocity_x = ((target_x - snowman.x) / dt).clamp(-2.0, 2.0);
        snowman.drag_velocity_y = ((target_y - snowman.y) / dt).clamp(-2.0, 2.0);
        snowman.x = target_x;
        snowman.y = target_y;
        snowman.held = true;
        true
    }

    pub fn release_snowman(&mut self, id: u64) -> bool {
        let Some(index) = self.snowmen.iter().position(|snowman| snowman.id == id) else {
            return false;
        };
        let snowman = &mut self.snowmen[index];
        snowman.held = false;
        snowman.velocity_x = snowman.drag_velocity_x.clamp(-0.6, 0.6);
        snowman.velocity_y = snowman.drag_velocity_y.clamp(-0.8, 0.8);
        snowman.drag_velocity_x = 0.0;
        snowman.drag_velocity_y = 0.0;
        true
    }

    pub fn clear_snow_objects(&mut self) {
        self.snowballs.clear();
        self.snowmen.clear();
    }

    pub fn set_accumulation_enabled(&mut self, enabled: bool) {
        self.accumulation_enabled = enabled;
    }

    /// Display width divided by height. Snow depths are normalized to the display
    /// height while distances along a window are normalized to its width, so the
    /// angle of repose needs the aspect ratio to compare the two.
    pub fn set_display_aspect(&mut self, aspect: f32) {
        if aspect.is_finite() && aspect > 0.0 {
            self.display_aspect = aspect.clamp(0.5, 4.0);
        }
    }

    pub fn set_accumulation_speed(&mut self, speed: f32) {
        if speed.is_finite() {
            self.accumulation_speed = speed.clamp(0.0, 1.0);
        }
    }

    pub fn set_window_wind_draft_enabled(&mut self, enabled: bool) {
        if self.window_wind_draft_enabled == enabled {
            return;
        }
        self.window_wind_draft_enabled = enabled;
        for window_index in 0..self.draft_windows.len() {
            self.draft_windows[window_index].next_gust_at =
                self.elapsed + self.random_draft_interval();
        }
        if !enabled {
            for particle in &mut self.particles {
                particle.window_draft = false;
                particle.window_draft_entered = false;
                particle.draft_window_id = 0;
                particle.draft_velocity_x = 0.0;
                particle.draft_velocity_y = 0.0;
            }
        }
    }

    pub fn set_wind_draft_regions(&mut self, regions: &[BlockedRegion]) {
        let mut updated = Vec::with_capacity(regions.len());
        for region in regions.iter().copied() {
            if !region.x.is_finite()
                || !region.y.is_finite()
                || !region.width.is_finite()
                || !region.height.is_finite()
                || region.width <= 0.0
                || region.height <= 0.0
                || region.x < 0.0
                || region.y < 0.0
                || region.x + region.width > 1.001
                || region.y + region.height > 1.001
                || updated
                    .iter()
                    .any(|window: &DraftWindow| window.id == region.id)
            {
                continue;
            }
            let next_gust_at = self
                .draft_windows
                .iter()
                .find(|window| window.id == region.id)
                .map(|window| window.next_gust_at);
            let next_gust_at =
                next_gust_at.unwrap_or_else(|| self.elapsed + self.random_draft_interval());
            updated.push(DraftWindow {
                id: region.id,
                region,
                next_gust_at,
            });
        }
        self.draft_windows = updated;
    }

    fn random_draft_interval(&mut self) -> f32 {
        8.0 + self.random() * 17.0
    }

    fn update_window_wind_drafts(&mut self) {
        if !self.window_wind_draft_enabled {
            return;
        }

        for window_index in 0..self.draft_windows.len() {
            let window = self.draft_windows[window_index];
            if self.elapsed < window.next_gust_at {
                continue;
            }
            self.draft_windows[window_index].next_gust_at =
                self.elapsed + self.random_draft_interval();
            if self.random() < 0.35 {
                continue;
            }

            let direction = if self.random() < 0.5 { 1.0 } else { -1.0 };
            let gust_y = window.region.y + window.region.height * (0.15 + self.random() * 0.7);
            let edge_x = if direction > 0.0 {
                window.region.x
            } else {
                window.region.x + window.region.width
            };
            let mut affected = 0;
            for particle_index in 0..self.particles.len() {
                let particle = self.particles[particle_index];
                let outside_distance = if direction > 0.0 {
                    edge_x - particle.x
                } else {
                    particle.x - edge_x
                };
                if !(0.0..=0.025).contains(&outside_distance) || (particle.y - gust_y).abs() > 0.055
                {
                    continue;
                }

                let draft_velocity_x = direction * (0.022 + self.random() * 0.012);
                let draft_velocity_y = (self.random() - 0.5) * 0.012;
                let particle = &mut self.particles[particle_index];
                particle.draft_velocity_x = draft_velocity_x;
                particle.draft_velocity_y = draft_velocity_y;
                particle.window_draft = true;
                particle.window_draft_entered = false;
                particle.draft_window_id = window.id;
                affected += 1;
                if affected >= 3 {
                    break;
                }
            }
        }
    }

    pub fn set_particle_count(&mut self, count: usize) {
        let count = count.clamp(1, 20_000);
        self.particles.truncate(count);
        while self.particles.len() < count {
            let particle = self.spawn(false);
            self.particles.push(particle);
        }
    }

    pub fn set_surfaces(&mut self, surfaces: &[CollisionSurface]) {
        let mut updated = Vec::with_capacity(surfaces.len().min(MAX_ACCUMULATION_SURFACES));
        for surface in surfaces.iter().copied().take(MAX_ACCUMULATION_SURFACES) {
            if !surface.x.is_finite()
                || !surface.y.is_finite()
                || !surface.width.is_finite()
                || !surface.edge_margin.is_finite()
                || !surface.maximum_depth.is_finite()
                || surface.width <= 0.0
                || surface.edge_margin < 0.0
                || surface.edge_margin * 2.0 >= surface.width
                || surface.maximum_depth <= 0.0
                || !(0.0..=1.0).contains(&surface.x)
                || !(0.0..=1.0).contains(&surface.y)
                || match surface.orientation {
                    SurfaceOrientation::Horizontal => surface.x + surface.width > 1.001,
                    SurfaceOrientation::VerticalToRight | SurfaceOrientation::VerticalToLeft => {
                        surface.y + surface.width > 1.001
                    }
                }
            {
                continue;
            }
            if updated
                .iter()
                .any(|existing: &SnowSurface| existing.id == surface.id)
            {
                continue;
            }
            let existing = self.snow_surfaces.iter().find(|existing| {
                existing.id == surface.id && existing.orientation == surface.orientation
            });
            let persistent =
                surface.persistent && existing.is_none_or(|existing| existing.persistent);
            let fresh_depth =
                if persistent && self.accumulation_enabled && self.accumulation_speed > 0.0 {
                    INITIAL_DOCK_SNOW_DEPTH
                } else {
                    0.0
                };
            let same_shape = existing.is_some_and(|existing| {
                (existing.width - surface.width).abs() <= SAME_WIDTH_TOLERANCE
            });
            let inherited = if same_shape {
                // The surface only moved: the profile travels with it unchanged.
                existing.map(|existing| existing.snow_depths.clone())
            } else {
                // A window edge was split, joined or resized (for example another window
                // now covers part of it): keep the snow where it lies on screen.
                resample_snow_depths(&self.snow_surfaces, existing, &surface)
            };
            let snow_depths = inherited
                .or_else(|| existing.map(|existing| existing.snow_depths.clone()))
                .unwrap_or_else(|| vec![fresh_depth; ACCUMULATION_BINS]);
            let maximum_depth = surface.maximum_depth;
            let snow_depths = snow_depths
                .into_iter()
                .map(|depth| depth.min(SCULPTED_SNOW_DEPTH_LIMIT))
                .collect();
            updated.push(SnowSurface {
                id: surface.id,
                x: surface.x,
                y: surface.y,
                width: surface.width,
                edge_margin: surface.edge_margin,
                orientation: surface.orientation,
                persistent,
                maximum_depth,
                snow_depths,
            });
        }
        self.snow_surfaces = updated;
    }

    fn snowball_candidate_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let mut candidate: Option<(usize, usize, f32)> = None;
        for (surface_index, surface) in self.snow_surfaces.iter().enumerate() {
            if surface.orientation != SurfaceOrientation::Horizontal
                || surface.width <= 0.0
                || x < surface.x + surface.edge_margin
                || x > surface.x + surface.width - surface.edge_margin
            {
                continue;
            }
            let bin = (((x - surface.x) / surface.width) * ACCUMULATION_BINS as f32)
                .floor()
                .clamp(0.0, (ACCUMULATION_BINS - 1) as f32) as usize;
            let depth = surface.snow_depths[bin];
            if depth < SNOWBALL_SPAWN_MIN_DEPTH {
                continue;
            }
            let top = surface.y - depth;
            if (y - top).abs() > 0.09 {
                continue;
            }
            if candidate.is_none_or(|(_, _, best_depth)| depth > best_depth) {
                candidate = Some((surface_index, bin, depth));
            }
        }
        candidate
    }

    /// True when the mitten could roll a snowball out of the drift under this
    /// point. The UI uses this to keep normal pointer movement side-effect free.
    /// True when no more snowballs can be created.
    pub fn snowball_limit_reached(&self) -> bool {
        self.snowballs.len() >= MAX_SNOWBALLS
    }

    pub fn can_roll_snowball_at(&self, x: f32, y: f32) -> bool {
        self.snowball_candidate_at(x, y).is_some()
    }

    /// Rolls exactly one snowball out of the drift under `(x, y)`. The UI calls
    /// this only for an explicit roll gesture, so ordinary sweeps never spawn.
    pub fn roll_snowball_at(&mut self, x: f32, y: f32, direction_x: f32) -> bool {
        if self.snowballs.len() >= MAX_SNOWBALLS || self.elapsed < self.next_snowball_spawn_at {
            return false;
        }
        let Some((surface_index, bin, _)) = self.snowball_candidate_at(x, y) else {
            return false;
        };
        let surface = &self.snow_surfaces[surface_index];
        let cost = (surface.snow_depths[bin] - 0.001).clamp(0.0, SNOWBALL_SNOW_COST);
        if cost <= 0.0 {
            return false;
        }
        self.snow_surfaces[surface_index].snow_depths[bin] -= cost;
        let surface = &self.snow_surfaces[surface_index];
        let surface_id = surface.id;
        let surface_x = surface.x;
        let surface_width = surface.width;
        let top = surface.y - surface.snow_depths[bin];
        let radius = SNOWBALL_RADIUS;
        let radius_x = radius / self.display_aspect;
        let min_x = surface_x + radius_x;
        let max_x = surface_x + surface_width - radius_x;
        let x = if max_x >= min_x {
            x.clamp(min_x, max_x)
        } else {
            surface_x + surface_width * 0.5
        }
        .clamp(radius_x, 1.0 - radius_x);
        let y = (top - radius).clamp(radius, 1.0 - radius);
        let direction = if direction_x.abs() > 0.1 {
            direction_x.signum()
        } else if self.random() < 0.5 {
            -1.0
        } else {
            1.0
        };
        let velocity_x = direction * (0.035 + self.random() * 0.03);
        let id = self.next_snowball_id;
        self.next_snowball_id = self.next_snowball_id.wrapping_add(1).max(1);
        self.snowballs.push(Snowball {
            id,
            x,
            y,
            velocity_x,
            velocity_y: 0.0,
            radius,
            state: SnowballState::Resting,
            group: 0,
            surface_id,
            surface_offset_x: x - surface_x,
            roll: 0.0,
            placed: false,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
        });
        self.next_snowball_spawn_at = self.elapsed + SNOWBALL_SPAWN_INTERVAL;
        true
    }

    fn detach_group_at(&mut self, index: usize) {
        let group = self.snowballs[index].group;
        self.snowballs[index].group = 0;
        if group == 0 {
            return;
        }
        for ball in &mut self.snowballs {
            if ball.group == group {
                ball.group = 0;
            }
        }
    }

    fn group_indices(&self) -> Vec<(u64, Vec<usize>)> {
        let mut groups: Vec<(u64, Vec<usize>)> = Vec::new();
        for (index, ball) in self.snowballs.iter().enumerate() {
            if ball.group == 0 {
                continue;
            }
            if let Some((_, indices)) = groups.iter_mut().find(|(group, _)| *group == ball.group) {
                indices.push(index);
            } else {
                groups.push((ball.group, vec![index]));
            }
        }
        groups
    }

    fn connect_snowballs(&mut self, first: u64, second: u64) {
        let Some(first_index) = self.snowballs.iter().position(|ball| ball.id == first) else {
            return;
        };
        let Some(second_index) = self.snowballs.iter().position(|ball| ball.id == second) else {
            return;
        };
        let first_group = self.snowballs[first_index].group;
        let second_group = self.snowballs[second_index].group;
        if first_group != 0 && first_group == second_group {
            return;
        }
        let group = match (first_group, second_group) {
            (0, 0) => first.min(second),
            (0, group) | (group, 0) => group,
            (first, second) => first.min(second),
        };
        for ball in &mut self.snowballs {
            if (first_group != 0 && ball.group == first_group)
                || (second_group != 0 && ball.group == second_group)
                || ball.id == first
                || ball.id == second
            {
                ball.group = group;
            }
        }
    }

    fn resolve_snowball_contacts(&mut self, broken: &mut Vec<u64>) -> Vec<(u64, u64)> {
        let aspect = self.display_aspect;
        let mut connections = Vec::new();
        for _ in 0..SNOWBALL_CONTACT_PASSES {
            for i in 0..self.snowballs.len() {
                if broken.contains(&self.snowballs[i].id) {
                    continue;
                }
                for j in (i + 1)..self.snowballs.len() {
                    if broken.contains(&self.snowballs[j].id) {
                        continue;
                    }
                    let first = self.snowballs[i];
                    let second = self.snowballs[j];
                    if first.state == SnowballState::Held || second.state == SnowballState::Held {
                        continue;
                    }
                    let delta_x = (first.x - second.x) * aspect;
                    let delta_y = first.y - second.y;
                    let radii = first.radius + second.radius;
                    if delta_x * delta_x + delta_y * delta_y
                        >= (radii - SNOWBALL_CONTACT_SLOP).max(0.0).powi(2)
                    {
                        continue;
                    }
                    let distance = (delta_x * delta_x + delta_y * delta_y)
                        .sqrt()
                        .max(f32::EPSILON);
                    let normal_x = delta_x / distance;
                    let normal_y = delta_y / distance;
                    let horizontal = (delta_x.abs() / radii).min(1.0);

                    let deliberate = first.placed || second.placed;
                    if horizontal < 0.55 && normal_y < -0.1 && deliberate {
                        // The first ball sits on top of the second one.
                        if first.velocity_y > SNOWBALL_BREAK_SPEED {
                            broken.push(first.id);
                            continue;
                        }
                        self.snowballs[i].y = second.y - second.radius - first.radius;
                        self.snowballs[i].velocity_y = 0.0;
                        self.snowballs[i].state = SnowballState::Resting;
                        self.snowballs[i].surface_id = NO_SURFACE_ID;
                        self.snowballs[i].surface_offset_x = 0.0;
                        self.snowballs[i].placed = false;
                        connections.push((first.id, second.id));
                    } else if horizontal < 0.55 && normal_y > 0.1 && deliberate {
                        // The second ball sits on top of the first one.
                        if second.velocity_y > SNOWBALL_BREAK_SPEED {
                            broken.push(second.id);
                            continue;
                        }
                        self.snowballs[j].y = first.y - first.radius - second.radius;
                        self.snowballs[j].velocity_y = 0.0;
                        self.snowballs[j].state = SnowballState::Resting;
                        self.snowballs[j].surface_id = NO_SURFACE_ID;
                        self.snowballs[j].surface_offset_x = 0.0;
                        self.snowballs[j].placed = false;
                        connections.push((first.id, second.id));
                    } else {
                        // Separate sideways. Pushing a resting ball down into
                        // the drift (or up out of it) every frame is what made
                        // side-by-side balls visibly twitch.
                        let current_dx = self.snowballs[i].x - self.snowballs[j].x;
                        let current_dy = self.snowballs[i].y - self.snowballs[j].y;
                        let minimum_dx =
                            ((radii * radii - current_dy * current_dy).max(0.0)).sqrt() / aspect;
                        if current_dx.abs() < minimum_dx {
                            let sign = if current_dx >= 0.0 { 1.0 } else { -1.0 };
                            let correction = (minimum_dx - current_dx.abs()) * 0.5;
                            self.snowballs[i].x += sign * correction;
                            self.snowballs[j].x -= sign * correction;
                        }
                        let first_radius_x = first.radius / aspect;
                        let second_radius_x = second.radius / aspect;
                        self.snowballs[i].x = self.snowballs[i]
                            .x
                            .clamp(first_radius_x, 1.0 - first_radius_x);
                        self.snowballs[j].x = self.snowballs[j]
                            .x
                            .clamp(second_radius_x, 1.0 - second_radius_x);

                        let relative_speed =
                            ((first.velocity_x - second.velocity_x) * aspect * normal_x
                                + (first.velocity_y - second.velocity_y) * normal_y)
                                .abs();
                        if relative_speed > SNOWBALL_BREAK_SPEED {
                            let first_speed = first.velocity_x.hypot(first.velocity_y);
                            let second_speed = second.velocity_x.hypot(second.velocity_y);
                            if first_speed >= second_speed {
                                broken.push(first.id);
                            } else {
                                broken.push(second.id);
                            }
                        } else {
                            // Inelastic horizontal contact: kill the closing
                            // speed without exciting vertical motion.
                            let closing =
                                (first.velocity_x - second.velocity_x) * aspect * normal_x;
                            if closing < 0.0 {
                                let average = (first.velocity_x + second.velocity_x) * 0.5;
                                self.snowballs[i].velocity_x = average;
                                self.snowballs[j].velocity_x = average;
                            }
                        }
                    }
                }
            }
        }
        connections
    }

    fn stabilize_snowball_groups(&mut self) {
        let groups = self.group_indices();
        for (_, indices) in groups {
            if indices.len() == 1 {
                self.snowballs[indices[0]].group = 0;
                continue;
            }
            if indices.len() != 2 {
                continue;
            }
            let (bottom, top) = if self.snowballs[indices[0]].y >= self.snowballs[indices[1]].y {
                (indices[0], indices[1])
            } else {
                (indices[1], indices[0])
            };
            if self.snowballs[top].state == SnowballState::Held {
                continue;
            }
            let bottom_ball = self.snowballs[bottom];
            let top_radius = self.snowballs[top].radius;
            self.snowballs[top].x = bottom_ball.x;
            self.snowballs[top].y =
                (bottom_ball.y - bottom_ball.radius - top_radius).max(top_radius);
            self.snowballs[top].velocity_x = 0.0;
            self.snowballs[top].velocity_y = 0.0;
            self.snowballs[top].state = SnowballState::Resting;
            self.snowballs[top].surface_id = NO_SURFACE_ID;
            self.snowballs[top].surface_offset_x = 0.0;
            if self.snowballs[bottom].state != SnowballState::Held {
                self.snowballs[bottom].velocity_y = 0.0;
                self.snowballs[bottom].state = SnowballState::Resting;
            }
        }
    }

    fn convert_snowman_groups(&mut self) {
        loop {
            let Some((_, indices)) = self
                .group_indices()
                .into_iter()
                .find(|(_, indices)| indices.len() >= 3)
            else {
                return;
            };
            if self.snowmen.len() >= MAX_SNOWMEN {
                return;
            }
            let chosen: Vec<usize> = indices.iter().copied().take(3).collect();
            let bottom_index = chosen
                .iter()
                .copied()
                .max_by(|first, second| {
                    (self.snowballs[*first].y + self.snowballs[*first].radius)
                        .total_cmp(&(self.snowballs[*second].y + self.snowballs[*second].radius))
                })
                .unwrap_or(chosen[0]);
            let bottom = self.snowballs[bottom_index];
            let average_x = chosen
                .iter()
                .map(|index| self.snowballs[*index].x)
                .sum::<f32>()
                / chosen.len() as f32;
            let average_radius = chosen
                .iter()
                .map(|index| self.snowballs[*index].radius)
                .sum::<f32>()
                / chosen.len() as f32;
            let radius = average_radius * 1.55;
            let radius_x = radius / self.display_aspect;
            let bottom_edge = chosen
                .iter()
                .map(|index| self.snowballs[*index].y + self.snowballs[*index].radius)
                .fold(0.0_f32, f32::max);
            let id = self.next_snowman_id;
            self.next_snowman_id = self.next_snowman_id.wrapping_add(1).max(1);
            self.snowmen.push(Snowman {
                id,
                x: average_x.clamp(radius_x, 1.0 - radius_x),
                y: bottom_edge.clamp(radius, 1.0),
                radius,
                surface_id: bottom.surface_id,
                surface_offset_x: bottom.surface_offset_x,
                held: false,
                velocity_x: 0.0,
                velocity_y: 0.0,
                drag_velocity_x: 0.0,
                drag_velocity_y: 0.0,
                drag_offset_x: 0.0,
                drag_offset_y: 0.0,
            });
            for index in indices
                .iter()
                .copied()
                .filter(|index| !chosen.contains(index))
            {
                self.snowballs[index].group = 0;
            }
            let mut chosen = chosen;
            chosen.sort_unstable();
            for index in chosen.into_iter().rev() {
                self.snowballs.remove(index);
            }
        }
    }

    fn remove_broken_snowballs(&mut self, broken: &[u64]) {
        for id in broken.iter().copied() {
            let Some(index) = self.snowballs.iter().position(|ball| ball.id == id) else {
                continue;
            };
            let ball = self.snowballs.remove(index);
            self.splash_snow(ball);
        }
        for (_, indices) in self.group_indices() {
            if indices.len() == 1 {
                self.snowballs[indices[0]].group = 0;
            }
        }
    }

    fn splash_snow(&mut self, ball: Snowball) {
        let ball_bottom = ball.y + ball.radius;
        let mut hit_surface: Option<(usize, f32)> = None;
        for (index, surface) in self.snow_surfaces.iter().enumerate() {
            if surface.orientation != SurfaceOrientation::Horizontal
                || ball.x < surface.x
                || ball.x > surface.x + surface.width
            {
                continue;
            }
            let top = surface.y - horizontal_depth_at(surface, ball.x);
            let distance = (top - ball_bottom).abs();
            if distance > 0.2 {
                continue;
            }
            if hit_surface.is_none_or(|(_, best)| distance < best) {
                hit_surface = Some((index, distance));
            }
        }
        if let Some((index, _)) = hit_surface {
            add_sculpted_snow(&mut self.snow_surfaces[index], ball.x, SNOWBALL_BREAK_DEPTH);
        }
        if self.particles.is_empty() {
            return;
        }
        for _ in 0..4 {
            let random_x = self.random();
            let random_y = self.random();
            let random_speed = self.random();
            let random_side = self.random();
            let index = ((self.random() * self.particles.len() as f32) as usize)
                .min(self.particles.len() - 1);
            let particle = &mut self.particles[index];
            particle.x = (ball.x + (random_x - 0.5) * ball.radius * 1.5).clamp(0.0, 1.0);
            particle.y = (ball.y + (random_y - 0.5) * ball.radius * 1.5).clamp(0.0, 1.0);
            particle.velocity_x = (random_side - 0.5) * 0.16;
            particle.velocity_y = -(0.02 + random_speed * 0.12);
            particle.window_draft = false;
            particle.window_draft_entered = false;
            particle.draft_window_id = 0;
            particle.draft_velocity_x = 0.0;
            particle.draft_velocity_y = 0.0;
        }
    }

    fn update_snowmen(&mut self, dt: f32) {
        let aspect = self.display_aspect;
        for index in 0..self.snowmen.len() {
            let mut snowman = self.snowmen[index];
            if snowman.held {
                continue;
            }
            if snowman.surface_id != NO_SURFACE_ID {
                if let Some(surface) = self.snow_surfaces.iter().find(|surface| {
                    surface.id == snowman.surface_id
                        && surface.orientation == SurfaceOrientation::Horizontal
                }) {
                    if snowman.surface_offset_x >= 0.0 && snowman.surface_offset_x <= surface.width
                    {
                        snowman.x = surface.x + snowman.surface_offset_x;
                        snowman.y = surface.y - horizontal_depth_at(surface, snowman.x);
                        snowman.velocity_x = 0.0;
                        snowman.velocity_y = 0.0;
                        self.snowmen[index] = snowman;
                        continue;
                    }
                }
                snowman.surface_id = NO_SURFACE_ID;
            }

            let radius_x = snowman.radius / aspect;
            let previous_y = snowman.y;
            snowman.velocity_x *= (-0.5 * dt).exp();
            snowman.velocity_y =
                (snowman.velocity_y + SNOWBALL_GRAVITY * dt).min(SNOWBALL_MAX_SPEED);
            snowman.x = (snowman.x + snowman.velocity_x * dt).clamp(radius_x, 1.0 - radius_x);
            snowman.y += snowman.velocity_y * dt;

            let mut support: Option<(u64, f32)> = None;
            for surface in self.snow_surfaces.iter() {
                if surface.orientation != SurfaceOrientation::Horizontal
                    || snowman.x < surface.x
                    || snowman.x > surface.x + surface.width
                {
                    continue;
                }
                let top = surface.y - horizontal_depth_at(surface, snowman.x);
                let crossing = (previous_y <= top + 0.01 && snowman.y >= top - 0.01)
                    || (snowman.y >= top - 0.01 && snowman.y <= surface.y + 0.05);
                if crossing && support.is_none_or(|(_, best)| top < best) {
                    support = Some((surface.id, top));
                }
            }
            for region in self.snowball_obstacles.iter() {
                if snowman.x < region.x || snowman.x > region.x + region.width {
                    continue;
                }
                let top = region.y;
                let crossing = (previous_y <= top + 0.01 && snowman.y >= top - 0.01)
                    || (snowman.y >= top - 0.01 && snowman.y <= region.y + 0.05);
                if crossing && support.is_none_or(|(_, best)| top < best) {
                    support = Some((NO_SURFACE_ID, top));
                }
            }
            if snowman.y >= 1.0 && support.is_none_or(|(_, top)| top > 1.0) {
                support = Some((NO_SURFACE_ID, 1.0));
            }

            if let Some((surface_id, top)) = support {
                snowman.y = top;
                snowman.velocity_y = 0.0;
                snowman.velocity_x *= 0.4;
                if surface_id != NO_SURFACE_ID {
                    if let Some(surface) = self
                        .snow_surfaces
                        .iter()
                        .find(|surface| surface.id == surface_id)
                    {
                        snowman.surface_offset_x = snowman.x - surface.x;
                    }
                }
                snowman.surface_id = surface_id;
            }
            snowman.x = snowman.x.clamp(radius_x, 1.0 - radius_x);
            snowman.y = snowman.y.clamp(snowman.radius * 4.7, 1.0);
            self.snowmen[index] = snowman;
        }
    }

    fn update_snowballs(&mut self, dt: f32) {
        if self.snowballs.is_empty() && self.snowmen.is_empty() {
            return;
        }
        let aspect = self.display_aspect;
        let mut broken: Vec<u64> = Vec::new();
        let mut index = 0;
        while index < self.snowballs.len() {
            let original = self.snowballs[index];
            if original.state == SnowballState::Held {
                index += 1;
                continue;
            }
            let radius = original.radius;
            let radius_x = radius / aspect;
            let mut x = original.x;
            let mut y = original.y;
            let mut velocity_x = original.velocity_x;
            let mut velocity_y = original.velocity_y;
            let mut state = SnowballState::Free;
            let mut surface_id = NO_SURFACE_ID;
            let mut surface_offset_x = 0.0;
            let mut roll = original.roll;
            let mut placed = original.placed;

            if original.state == SnowballState::Resting && original.surface_id != NO_SURFACE_ID {
                if let Some(surface) = self.snow_surfaces.iter().find(|surface| {
                    surface.id == original.surface_id
                        && surface.orientation == SurfaceOrientation::Horizontal
                }) {
                    if original.surface_offset_x >= 0.0
                        && original.surface_offset_x <= surface.width
                    {
                        x = surface.x + original.surface_offset_x;
                    }
                }
            }

            let previous_bottom = original.y + radius;
            velocity_y = (velocity_y + SNOWBALL_GRAVITY * dt).min(SNOWBALL_MAX_SPEED);
            velocity_x = velocity_x.clamp(-SNOWBALL_MAX_SPEED, SNOWBALL_MAX_SPEED);
            x += velocity_x * dt;
            y += velocity_y * dt;
            let bottom = y + radius;

            let mut support: Option<(u64, f32, f32)> = None;
            for surface in self.snow_surfaces.iter() {
                if surface.orientation != SurfaceOrientation::Horizontal
                    || x < surface.x
                    || x > surface.x + surface.width
                {
                    continue;
                }
                let top = surface.y - horizontal_depth_at(surface, x);
                let crossing = bottom >= top - 0.004 && previous_bottom <= top + 0.002;
                let resting = original.state == SnowballState::Resting
                    && original.surface_id == surface.id
                    && (bottom - top).abs() <= 0.03;
                if ((crossing && velocity_y >= -0.01) || resting)
                    && support.is_none_or(|(_, best, _)| top < best)
                {
                    support = Some((surface.id, top, x - surface.x));
                }
            }
            for region in self.snowball_obstacles.iter() {
                if x < region.x || x > region.x + region.width {
                    continue;
                }
                let top = region.y;
                let crossing = bottom >= top - 0.004 && previous_bottom <= top + 0.002;
                let resting =
                    original.state == SnowballState::Resting && (bottom - top).abs() <= 0.03;
                if ((crossing && velocity_y >= -0.01) || resting)
                    && support.is_none_or(|(_, best, _)| top < best)
                {
                    support = Some((NO_SURFACE_ID, top, 0.0));
                }
            }
            if y + radius >= 1.0 && support.is_none_or(|(_, top, _)| top > 1.0) {
                support = Some((NO_SURFACE_ID, 1.0, 0.0));
            }

            if let Some((found_surface, top, offset)) = support {
                if velocity_y > SNOWBALL_BREAK_SPEED {
                    broken.push(original.id);
                    index += 1;
                    continue;
                }
                let target_y = top - radius;
                if original.state == SnowballState::Resting && original.surface_id == found_surface
                {
                    // Snow under a resting ball can move while the shovel or
                    // mitten sweeps nearby. Follow it smoothly instead of
                    // snapping the ball up and down every frame.
                    let difference = target_y - y;
                    if difference.abs() > 0.00015 {
                        y += difference * 0.35;
                    } else {
                        y = target_y;
                    }
                } else {
                    y = target_y;
                }
                velocity_y = 0.0;
                state = SnowballState::Resting;
                placed = false;
                surface_id = found_surface;
                surface_offset_x = if found_surface != NO_SURFACE_ID {
                    offset
                } else {
                    0.0
                };
                let friction = (-SNOWBALL_FRICTION * dt).exp();
                velocity_x *= friction;
                if velocity_x.abs() < SNOWBALL_REST_SPEED {
                    velocity_x = 0.0;
                }
                x += velocity_x * dt;
                if radius_x > 0.0001 {
                    roll += velocity_x * dt / radius_x;
                }
                // The surface may itself have moved this frame; store the offset
                // after rolling so the ball keeps following the window next frame.
                if found_surface != NO_SURFACE_ID {
                    if let Some(surface) = self
                        .snow_surfaces
                        .iter()
                        .find(|surface| surface.id == found_surface)
                    {
                        surface_offset_x = x - surface.x;
                    }
                }
            }

            if x < radius_x {
                x = radius_x;
                if velocity_x.abs() > SNOWBALL_BREAK_SPEED {
                    broken.push(original.id);
                    index += 1;
                    continue;
                }
                velocity_x = velocity_x.abs() * 0.25;
            } else if x > 1.0 - radius_x {
                x = 1.0 - radius_x;
                if velocity_x.abs() > SNOWBALL_BREAK_SPEED {
                    broken.push(original.id);
                    index += 1;
                    continue;
                }
                velocity_x = -velocity_x.abs() * 0.25;
            }
            if y < radius {
                y = radius;
                velocity_y = velocity_y.abs() * 0.2;
            }
            if y > 1.0 - radius {
                if velocity_y > SNOWBALL_BREAK_SPEED {
                    broken.push(original.id);
                    index += 1;
                    continue;
                }
                y = 1.0 - radius;
                velocity_y = 0.0;
                state = SnowballState::Resting;
            }

            let mut hit_hard = false;
            for region in self.snowball_obstacles.iter() {
                let left = region.x;
                let right = region.x + region.width;
                let top = region.y;
                let bottom = region.y + region.height;
                if y <= top || y >= bottom - radius * 0.25 {
                    continue;
                }
                if x + radius_x <= left || x - radius_x >= right {
                    continue;
                }
                if original.x <= left && x + radius_x > left {
                    if velocity_x > SNOWBALL_BREAK_SPEED {
                        hit_hard = true;
                        break;
                    }
                    x = left - radius_x;
                    velocity_x = -velocity_x.abs() * 0.3;
                    state = SnowballState::Free;
                    surface_id = NO_SURFACE_ID;
                } else if original.x >= right && x - radius_x < right {
                    if velocity_x < -SNOWBALL_BREAK_SPEED {
                        hit_hard = true;
                        break;
                    }
                    x = right + radius_x;
                    velocity_x = velocity_x.abs() * 0.3;
                    state = SnowballState::Free;
                    surface_id = NO_SURFACE_ID;
                } else if x < left + (right - left) * 0.5 {
                    x = left - radius_x;
                    velocity_x = -velocity_x.abs() * 0.3;
                } else {
                    x = right + radius_x;
                    velocity_x = velocity_x.abs() * 0.3;
                }
            }
            if hit_hard {
                broken.push(original.id);
                index += 1;
                continue;
            }

            let snowmen: Vec<Snowman> = self.snowmen.to_vec();
            let mut touched_snowman = false;
            for _ in 0..3 {
                for snowman in &snowmen {
                    if push_out_of_snowman(&mut x, &mut y, radius, snowman, aspect) {
                        touched_snowman = true;
                    }
                }
                x = x.clamp(radius_x, 1.0 - radius_x);
                y = y.clamp(radius, 1.0 - radius);
            }
            if touched_snowman {
                velocity_x *= 0.35;
                velocity_y = 0.0;
                state = SnowballState::Free;
                surface_id = NO_SURFACE_ID;
            }

            self.snowballs[index] = Snowball {
                x,
                y,
                velocity_x,
                velocity_y,
                state,
                surface_id,
                surface_offset_x,
                roll,
                placed,
                ..original
            };
            index += 1;
        }

        let connections = self.resolve_snowball_contacts(&mut broken);
        self.remove_broken_snowballs(&broken);
        for (first, second) in connections {
            self.connect_snowballs(first, second);
        }
        self.stabilize_snowball_groups();
        self.convert_snowman_groups();
        self.update_snowmen(dt);
    }

    pub fn clear_snow(&mut self) {
        for surface in &mut self.snow_surfaces {
            surface.snow_depths.fill(0.0);
            surface.persistent = false;
        }
        self.clear_snow_objects();
    }

    pub fn brush_snow_at(&mut self, x: f32, y: f32, radius_x: f32, radius_y: f32, strength: f32) {
        if !x.is_finite()
            || !y.is_finite()
            || !radius_x.is_finite()
            || !radius_y.is_finite()
            || !strength.is_finite()
            || radius_x <= 0.0
            || radius_y <= 0.0
            || strength <= 0.0
        {
            return;
        }

        let strength = strength.clamp(0.0, 1.0);
        for surface in &mut self.snow_surfaces {
            for bin in 0..surface.snow_depths.len() {
                let depth = surface.snow_depths[bin];
                if depth <= 0.0 {
                    continue;
                }
                let (snow_x, snow_y) = surface_profile_position(surface, bin, depth);
                let distance = ((snow_x - x) / radius_x).hypot((snow_y - y) / radius_y);
                if distance < 1.0 {
                    surface.persistent = false;
                    if strength >= 1.0 {
                        surface.snow_depths[bin] = 0.0;
                        continue;
                    }
                    let influence = 1.0 - distance;
                    let new_depth = depth * (1.0 - strength * influence);
                    surface.snow_depths[bin] = if new_depth < 0.0001 { 0.0 } else { new_depth };
                }
            }
        }
    }

    pub fn push_snow_at(&mut self, stroke: BrushStroke) {
        if !stroke.x.is_finite()
            || !stroke.y.is_finite()
            || !stroke.radius_x.is_finite()
            || !stroke.radius_y.is_finite()
            || !stroke.strength.is_finite()
            || !stroke.direction_x.is_finite()
            || !stroke.direction_y.is_finite()
            || stroke.radius_x <= 0.0
            || stroke.radius_y <= 0.0
            || stroke.strength <= 0.0
        {
            return;
        }

        let direction_length = stroke
            .direction_x
            .hypot(stroke.direction_y)
            .max(f32::EPSILON);
        let stroke = BrushStroke {
            strength: stroke.strength.clamp(0.0, 1.0),
            direction_x: stroke.direction_x / direction_length,
            direction_y: stroke.direction_y / direction_length,
            ..stroke
        };
        self.push_particles_at(stroke);
        self.push_accumulated_snow_at(stroke);
    }

    fn push_particles_at(&mut self, stroke: BrushStroke) {
        for particle in &mut self.particles {
            let offset_x = (particle.x - stroke.x) / stroke.radius_x;
            let offset_y = (particle.y - stroke.y) / stroke.radius_y;
            let distance = offset_x.hypot(offset_y);
            if distance >= 1.0 {
                continue;
            }

            let falloff = (1.0 - distance).powi(2);
            let radial_length = distance.max(0.15);
            let radial_x = offset_x / radial_length;
            let radial_y = offset_y / radial_length;
            let push_x = stroke.direction_x * 0.75 + radial_x * 0.25;
            let push_y = stroke.direction_y * 0.75 + radial_y * 0.25;
            let impulse = 0.12 * stroke.strength * falloff;
            particle.velocity_x = (particle.velocity_x + push_x * impulse).clamp(-0.3, 0.3);
            particle.velocity_y =
                (particle.velocity_y + push_y * impulse).clamp(-0.08, particle.terminal_speed);
        }
        for snowball in &mut self.snowballs {
            if snowball.state == SnowballState::Held {
                continue;
            }
            let offset_x = (snowball.x - stroke.x) / stroke.radius_x;
            let offset_y = (snowball.y - stroke.y) / stroke.radius_y;
            let distance = offset_x.hypot(offset_y);
            if distance >= 1.0 {
                continue;
            }
            let falloff = (1.0 - distance).powi(2);
            let impulse = 0.18 * stroke.strength * falloff;
            snowball.velocity_x = (snowball.velocity_x + stroke.direction_x * impulse)
                .clamp(-SNOWBALL_MAX_SPEED, SNOWBALL_MAX_SPEED);
            snowball.velocity_y = (snowball.velocity_y + stroke.direction_y * impulse)
                .clamp(-SNOWBALL_MAX_SPEED, SNOWBALL_MAX_SPEED);
            snowball.state = SnowballState::Free;
            snowball.surface_id = NO_SURFACE_ID;
            snowball.surface_offset_x = 0.0;
        }
    }

    fn push_accumulated_snow_at(&mut self, stroke: BrushStroke) {
        // (x, y, direction) of flakes released where snow was swept off a surface end.
        let mut spilled_flakes: Vec<(f32, f32, f32)> = Vec::new();
        for surface in &mut self.snow_surfaces {
            let mut spilled_left = 0.0_f32;
            let mut spilled_right = 0.0_f32;
            let first_bin =
                ((surface.edge_margin / surface.width * ACCUMULATION_BINS as f32).ceil() as usize)
                    .min(ACCUMULATION_BINS - 1);
            let last_bin = (((surface.width - surface.edge_margin) / surface.width
                * ACCUMULATION_BINS as f32)
                .ceil() as usize)
                .saturating_sub(1)
                .max(first_bin)
                .min(ACCUMULATION_BINS - 1);
            let mut changes = [0.0; ACCUMULATION_BINS];
            let vertical = surface.orientation != SurfaceOrientation::Horizontal;
            let stroke_axis = if vertical {
                stroke.direction_y
            } else {
                stroke.direction_x
            };
            let push_direction = if stroke_axis.abs() > 0.15 {
                stroke_axis.signum() as isize
            } else {
                0
            };

            for source_bin in first_bin..=last_bin {
                let depth = surface.snow_depths[source_bin];
                if depth <= 0.0 {
                    continue;
                }
                let (snow_x, snow_y) = surface_profile_position(surface, source_bin, depth);
                let distance = ((snow_x - stroke.x) / stroke.radius_x)
                    .hypot((snow_y - stroke.y) / stroke.radius_y);
                if distance >= 1.0 {
                    continue;
                }

                let profile_cursor = if vertical { stroke.y } else { stroke.x };
                let profile_position = if vertical { snow_y } else { snow_x };
                let direction = if push_direction != 0 {
                    push_direction
                } else if profile_position >= profile_cursor {
                    1
                } else {
                    -1
                };
                let falloff = (1.0 - distance).powi(2);
                let requested = depth * stroke.strength * falloff * 0.85;
                let mut remaining = requested;
                for (offset, weight) in [(1_isize, 0.55_f32), (2, 0.3), (3, 0.15)] {
                    let target_bin = source_bin as isize + direction * offset;
                    if target_bin < first_bin as isize || target_bin > last_bin as isize {
                        // Swept past the end of a window top: the snow falls off the edge.
                        // Dock sides keep their old behaviour (snow stays put).
                        if vertical {
                            continue;
                        }
                        let accepted = remaining.min(requested * weight);
                        if accepted > 0.0 {
                            remaining -= accepted;
                            if direction > 0 {
                                spilled_right += accepted;
                            } else {
                                spilled_left += accepted;
                            }
                        }
                        if remaining <= f32::EPSILON {
                            break;
                        }
                        continue;
                    }
                    let target_bin = target_bin as usize;
                    let projected_depth = (surface.snow_depths[target_bin] + changes[target_bin])
                        .clamp(0.0, SCULPTED_SNOW_DEPTH_LIMIT);
                    let room = SCULPTED_SNOW_DEPTH_LIMIT - projected_depth;
                    let accepted = remaining.min(requested * weight).min(room);
                    if accepted > 0.0 {
                        changes[target_bin] += accepted;
                        remaining -= accepted;
                    }
                    if remaining <= f32::EPSILON {
                        break;
                    }
                }
                let moved = requested - remaining;
                changes[source_bin] -= moved;
            }

            for (bin, change) in changes
                .iter()
                .copied()
                .enumerate()
                .take(last_bin + 1)
                .skip(first_bin)
            {
                surface.snow_depths[bin] =
                    (surface.snow_depths[bin] + change).clamp(0.0, SCULPTED_SNOW_DEPTH_LIMIT);
            }

            for (spilled, last, direction) in [
                (spilled_right, true, 1.0_f32),
                (spilled_left, false, -1.0_f32),
            ] {
                let flakes = ((spilled / SPILL_FLAKE_VOLUME).round() as usize)
                    .min(MAX_SPILL_FLAKES_PER_STROKE);
                if flakes == 0 {
                    continue;
                }
                let (end_x, end_bin) = if last {
                    (surface.x + surface.width - surface.edge_margin, last_bin)
                } else {
                    (surface.x + surface.edge_margin, first_bin)
                };
                let end_y = surface.y - surface.snow_depths[end_bin];
                for _ in 0..flakes {
                    spilled_flakes.push((end_x, end_y, direction));
                }
            }
        }

        // Spilled snow re-enters the pool as falling flakes just beyond the edge. The
        // pool size never changes: existing flakes are recycled into the spilled ones.
        if self.particles.is_empty() {
            return;
        }
        for (x, y, direction) in spilled_flakes {
            let index = ((self.random() * self.particles.len() as f32) as usize)
                .min(self.particles.len() - 1);
            let outward = 0.012 + self.random() * 0.02;
            let downward = 0.01 + self.random() * 0.02;
            let particle = &mut self.particles[index];
            particle.x = x + direction * 0.002;
            particle.y = y;
            particle.velocity_x = direction * outward;
            particle.velocity_y = downward;
            particle.window_draft = false;
            particle.window_draft_entered = false;
            particle.draft_window_id = 0;
            particle.draft_velocity_x = 0.0;
            particle.draft_velocity_y = 0.0;
        }
    }

    pub fn set_config(&mut self, config: PhysicsConfig) {
        self.config = validate_config(config);
    }

    pub fn update(&mut self, delta_seconds: f32) {
        if !delta_seconds.is_finite() || delta_seconds <= 0.0 {
            return;
        }
        let dt = delta_seconds.min(0.05);
        self.elapsed += dt;
        self.update_window_wind_drafts();
        let accumulation_active = self.accumulation_enabled && self.accumulation_speed > 0.0;
        if accumulation_active {
            for surface in &mut self.snow_surfaces {
                for depth in &mut surface.snow_depths {
                    let minimum = if surface.persistent {
                        INITIAL_DOCK_SNOW_DEPTH
                    } else {
                        0.0
                    };
                    *depth = (*depth - SNOW_MELT_RATE * dt).max(minimum);
                }
            }
        }
        let wind_shape = if self.config.wind_alternates {
            let swing = (self.elapsed * 0.85).sin();
            if swing >= 0.0 {
                0.55 + 0.45 * swing
            } else {
                -(0.55 + 0.45 * -swing)
            }
        } else {
            0.78 + 0.22 * self.elapsed.mul_add(0.17, 0.0).sin()
        };
        let global_wind = wind_shape * self.config.wind_strength;

        for index in 0..self.particles.len() {
            let previous_x = self.particles[index].x;
            let previous_y = self.particles[index].y;
            let particle = &mut self.particles[index];
            let wave = (self.elapsed * 1.65 + particle.phase).sin();
            let gust = (self.elapsed * 0.57 + particle.phase * 1.73).cos();
            let turbulence = wave * gust * self.config.turbulence;
            particle.velocity_x +=
                (global_wind * particle.wind_influence * self.config.wind_direction_x + turbulence)
                    * dt;
            particle.velocity_x = particle.velocity_x.clamp(-0.09, 0.09);
            particle.velocity_y = (particle.velocity_y
                + (self.config.gravity
                    + global_wind * particle.wind_influence * self.config.wind_direction_y)
                    * dt)
                .clamp(-0.08, particle.terminal_speed);
            particle.x += (particle.velocity_x + particle.draft_velocity_x) * dt;
            particle.y += (particle.velocity_y + particle.draft_velocity_y) * dt;
            let draft_decay = (-2.0 * dt).exp();
            particle.draft_velocity_x *= draft_decay;
            particle.draft_velocity_y *= draft_decay;
            particle.rotation += particle.rotation_speed * dt;

            let collision = accumulation_active
                .then(|| {
                    self.snow_surfaces
                        .iter()
                        .enumerate()
                        .find_map(|(surface_index, surface)| match surface.orientation {
                            SurfaceOrientation::Horizontal => {
                                let accumulation_left = surface.x + surface.edge_margin;
                                let accumulation_right =
                                    surface.x + surface.width - surface.edge_margin;
                                if particle.x < accumulation_left
                                    || particle.x >= accumulation_right
                                {
                                    return None;
                                }
                                let bin = (((particle.x - surface.x) / surface.width)
                                    * ACCUMULATION_BINS as f32)
                                    .floor() as usize;
                                let bin = bin.min(ACCUMULATION_BINS - 1);
                                let landing_y = surface.y - surface.snow_depths[bin];
                                (previous_y <= landing_y && particle.y >= landing_y)
                                    .then_some((surface_index, bin))
                            }
                            SurfaceOrientation::VerticalToRight
                            | SurfaceOrientation::VerticalToLeft => {
                                let accumulation_top = surface.y + surface.edge_margin;
                                let accumulation_bottom =
                                    surface.y + surface.width - surface.edge_margin;
                                if particle.y < accumulation_top
                                    || particle.y >= accumulation_bottom
                                {
                                    return None;
                                }
                                let moving_into_surface = match surface.orientation {
                                    SurfaceOrientation::VerticalToRight => {
                                        previous_x >= surface.x && particle.x <= surface.x
                                    }
                                    SurfaceOrientation::VerticalToLeft => {
                                        previous_x <= surface.x && particle.x >= surface.x
                                    }
                                    SurfaceOrientation::Horizontal => false,
                                };
                                let bin = (((particle.y - surface.y) / surface.width)
                                    * ACCUMULATION_BINS as f32)
                                    .floor() as usize;
                                moving_into_surface
                                    .then_some((surface_index, bin.min(ACCUMULATION_BINS - 1)))
                            }
                        })
                })
                .flatten();
            if let Some((surface_index, bin)) = collision {
                let deposit =
                    SNOW_DEPOSIT_DEPTH * self.accumulation_speed * (0.85 + self.random() * 0.3);
                let maximum_depth = self.snow_surfaces[surface_index]
                    .maximum_depth
                    .min(MAX_SNOW_DEPTH);
                let first_bin = ((self.snow_surfaces[surface_index].edge_margin
                    / self.snow_surfaces[surface_index].width
                    * ACCUMULATION_BINS as f32)
                    .floor() as isize)
                    .clamp(0, ACCUMULATION_BINS as isize - 1);
                let last_bin = (((self.snow_surfaces[surface_index].width
                    - self.snow_surfaces[surface_index].edge_margin)
                    / self.snow_surfaces[surface_index].width
                    * ACCUMULATION_BINS as f32)
                    .ceil() as isize
                    - 1)
                .clamp(first_bin, ACCUMULATION_BINS as isize - 1);
                for (offset, weight) in [
                    (0_isize, 0.32_f32),
                    (-1, 0.22),
                    (1, 0.22),
                    (-2, 0.1),
                    (2, 0.1),
                    (-3, 0.02),
                    (3, 0.02),
                ] {
                    let neighbor = (bin as isize + offset).clamp(first_bin, last_bin) as usize;
                    let depth = &mut self.snow_surfaces[surface_index].snow_depths[neighbor];
                    // Fresh snowfall stops at the untouched surface limit. A pile
                    // sculpted by the mitten may already be taller, and new flakes
                    // must not add to it (nor compress it back down).
                    if *depth < maximum_depth {
                        *depth = (*depth + deposit * weight).min(maximum_depth);
                    }
                }
                self.particles[index] = self.spawn(false);
                continue;
            }

            if particle.x < -0.02 {
                particle.x = 1.02;
            } else if particle.x > 1.02 {
                particle.x = -0.02;
            }
            if particle.y > 1.02 {
                self.particles[index] = self.spawn(false);
                continue;
            }
            if particle.window_draft {
                if let Some(window) = self
                    .draft_windows
                    .iter()
                    .find(|window| window.id == particle.draft_window_id)
                {
                    let inside = particle.x >= window.region.x
                        && particle.x < window.region.x + window.region.width
                        && particle.y >= window.region.y
                        && particle.y < window.region.y + window.region.height;
                    if inside {
                        particle.window_draft_entered = true;
                    } else if particle.window_draft_entered {
                        particle.window_draft = false;
                        particle.window_draft_entered = false;
                        particle.draft_window_id = 0;
                    }
                } else {
                    particle.window_draft = false;
                    particle.window_draft_entered = false;
                    particle.draft_window_id = 0;
                    particle.draft_velocity_x = 0.0;
                    particle.draft_velocity_y = 0.0;
                }
            }
        }

        if accumulation_active {
            for surface in &mut self.snow_surfaces {
                for _ in 0..2 {
                    let mut relaxed = [0.0; ACCUMULATION_BINS];
                    for (bin, depth) in surface.snow_depths.iter().copied().enumerate() {
                        let left = surface.snow_depths[bin.saturating_sub(1)];
                        let right = surface.snow_depths[(bin + 1).min(ACCUMULATION_BINS - 1)];
                        let local_average = (left + depth + right) / 3.0;
                        // Naturally fallen drifts may relax fully; a pile packed by
                        // the mitten keeps most of its height and only softens a bit.
                        let blend = if depth > surface.maximum_depth {
                            0.12
                        } else {
                            0.55
                        };
                        relaxed[bin] = (depth + (local_average - depth) * blend)
                            .clamp(0.0, SCULPTED_SNOW_DEPTH_LIMIT);
                    }
                    surface.snow_depths.copy_from_slice(&relaxed);
                }
                slump_snow(surface, self.display_aspect);
            }
        }
        self.update_snowballs(dt);
    }

    fn spawn(&mut self, distribute_vertically: bool) -> Particle {
        let depth = self.random();
        let diameter = 2.0 + depth * 4.0;
        let terminal_speed = self.config.minimum_terminal_speed
            + (self.config.maximum_terminal_speed - self.config.minimum_terminal_speed)
                * (0.25 + depth * 0.75);
        let y = if distribute_vertically {
            self.random() * 1.04 - 0.02
        } else {
            0.0
        };

        Particle {
            x: self.random(),
            y,
            velocity_x: (self.random() - 0.5) * 0.012,
            velocity_y: terminal_speed * (0.2 + self.random() * 0.6),
            terminal_speed,
            diameter,
            rotation: self.random() * std::f32::consts::TAU,
            rotation_speed: (self.random() - 0.5) * 1.4,
            opacity: 0.35 + depth * 0.55,
            depth,
            phase: self.random() * std::f32::consts::TAU,
            wind_influence: 0.3 + self.random() * 0.7,
            window_draft: false,
            window_draft_entered: false,
            draft_window_id: 0,
            draft_velocity_x: 0.0,
            draft_velocity_y: 0.0,
        }
    }

    fn random(&mut self) -> f32 {
        self.random_state ^= self.random_state << 13;
        self.random_state ^= self.random_state >> 17;
        self.random_state ^= self.random_state << 5;
        (self.random_state >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

/// Linearly interpolated snow depth of `donor` at the absolute position `x`,
/// or `None` when `x` is outside the donor.
fn sample_snow_depth(donor: &SnowSurface, x: f32) -> Option<f32> {
    let position = (x - donor.x) / donor.width * ACCUMULATION_BINS as f32 - 0.5;
    if position < -0.5 || position > ACCUMULATION_BINS as f32 - 0.5 {
        return None;
    }
    let position = position.clamp(0.0, (ACCUMULATION_BINS - 1) as f32);
    let lower = position.floor() as usize;
    let upper = (lower + 1).min(ACCUMULATION_BINS - 1);
    let fraction = position - lower as f32;
    Some(donor.snow_depths[lower] * (1.0 - fraction) + donor.snow_depths[upper] * fraction)
}

/// Rebuilds the snow profile of a horizontal surface whose extent changed from the
/// previous surfaces of the same window (ids share their upper 32 bits) lying on the
/// same row. Returns `None` when the situation is ambiguous and the caller should
/// fall back to the old behaviour.
fn resample_snow_depths(
    previous: &[SnowSurface],
    existing: Option<&SnowSurface>,
    surface: &CollisionSurface,
) -> Option<Vec<f32>> {
    if surface.orientation != SurfaceOrientation::Horizontal {
        return None;
    }
    // A matching surface that moved as well as changed size cannot be told apart from a
    // window drag, so only edges that stay on the same screen row are resampled.
    if let Some(existing) = existing {
        let left_fixed = (existing.x - surface.x).abs() <= SAME_ROW_TOLERANCE;
        let right_fixed = ((existing.x + existing.width) - (surface.x + surface.width)).abs()
            <= SAME_ROW_TOLERANCE;
        if !left_fixed && !right_fixed {
            return None;
        }
    }
    let owner = surface.id >> 32;
    let donors: Vec<&SnowSurface> = previous
        .iter()
        .filter(|donor| {
            donor.orientation == SurfaceOrientation::Horizontal
                && donor.id >> 32 == owner
                && (donor.y - surface.y).abs() <= SAME_ROW_TOLERANCE
                && donor.x < surface.x + surface.width
                && donor.x + donor.width > surface.x
        })
        .collect();
    if donors.is_empty() {
        return None;
    }
    Some(
        (0..ACCUMULATION_BINS)
            .map(|bin| {
                let x = surface.x + surface.width * (bin as f32 + 0.5) / ACCUMULATION_BINS as f32;
                donors
                    .iter()
                    .filter_map(|donor| sample_snow_depth(donor, x))
                    .fold(0.0_f32, f32::max)
            })
            .collect(),
    )
}

/// Lets steep piles of snow slump toward the angle of repose. Mass is conserved:
/// every transfer removes from one bin exactly what it adds to its neighbour.
fn slump_snow(surface: &mut SnowSurface, display_aspect: f32) {
    let first_bin = ((surface.edge_margin / surface.width * ACCUMULATION_BINS as f32).ceil()
        as usize)
        .min(ACCUMULATION_BINS - 1);
    let last_bin = (((surface.width - surface.edge_margin) / surface.width
        * ACCUMULATION_BINS as f32)
        .ceil() as usize)
        .saturating_sub(1)
        .max(first_bin)
        .min(ACCUMULATION_BINS - 1);
    if last_bin <= first_bin {
        return;
    }
    // Distance between neighbouring bins, in display-height units.
    let spacing = surface.width / ACCUMULATION_BINS as f32;
    let maximum_step = match surface.orientation {
        SurfaceOrientation::Horizontal => REPOSE_TANGENT * spacing * display_aspect,
        SurfaceOrientation::VerticalToRight | SurfaceOrientation::VerticalToLeft => {
            REPOSE_TANGENT * spacing / display_aspect
        }
    };
    for _ in 0..SLUMP_PASSES {
        let mut flow = [0.0_f32; ACCUMULATION_BINS];
        let mut moved_any = false;
        for bin in first_bin..last_bin {
            let difference = surface.snow_depths[bin] - surface.snow_depths[bin + 1];
            let excess = difference.abs() - maximum_step;
            if excess <= 0.0 {
                continue;
            }
            let moved = excess * 0.5 * SLUMP_RATE;
            moved_any = true;
            if difference > 0.0 {
                flow[bin] -= moved;
                flow[bin + 1] += moved;
            } else {
                flow[bin] += moved;
                flow[bin + 1] -= moved;
            }
        }
        if !moved_any {
            return;
        }
        for (bin, change) in flow.iter().enumerate().take(last_bin + 1).skip(first_bin) {
            surface.snow_depths[bin] =
                (surface.snow_depths[bin] + *change).clamp(0.0, SCULPTED_SNOW_DEPTH_LIMIT);
        }
    }
}

fn horizontal_depth_at(surface: &SnowSurface, x: f32) -> f32 {
    if surface.width <= 0.0 || surface.snow_depths.is_empty() {
        return 0.0;
    }
    let bin = (((x - surface.x) / surface.width) * ACCUMULATION_BINS as f32)
        .floor()
        .clamp(0.0, (ACCUMULATION_BINS - 1) as f32) as usize;
    surface.snow_depths[bin]
}

/// Adds snow from a broken snowball to the surface below it. This is sculpted
/// snow, so it may rise past the untouched surface limit.
fn add_sculpted_snow(surface: &mut SnowSurface, x: f32, amount: f32) {
    if surface.width <= 0.0 || surface.snow_depths.is_empty() || amount <= 0.0 {
        return;
    }
    let bin = (((x - surface.x) / surface.width) * ACCUMULATION_BINS as f32)
        .floor()
        .clamp(0.0, (ACCUMULATION_BINS - 1) as f32) as usize;
    for (offset, weight) in [(0_isize, 0.5_f32), (-1, 0.25), (1, 0.25)] {
        let target = (bin as isize + offset).clamp(0, ACCUMULATION_BINS as isize - 1) as usize;
        if surface.snow_depths[target] < SCULPTED_SNOW_DEPTH_LIMIT {
            surface.snow_depths[target] =
                (surface.snow_depths[target] + amount * weight).min(SCULPTED_SNOW_DEPTH_LIMIT);
        }
    }
}

fn surface_profile_position(surface: &SnowSurface, bin: usize, depth: f32) -> (f32, f32) {
    let progress = surface.width * (bin as f32 + 0.5) / ACCUMULATION_BINS as f32;
    match surface.orientation {
        SurfaceOrientation::Horizontal => (surface.x + progress, surface.y - depth),
        SurfaceOrientation::VerticalToRight => (surface.x + depth, surface.y + progress),
        SurfaceOrientation::VerticalToLeft => (surface.x - depth, surface.y + progress),
    }
}

fn validate_config(mut config: PhysicsConfig) -> PhysicsConfig {
    let defaults = PhysicsConfig::default();
    if !config.gravity.is_finite() || config.gravity < 0.0 {
        config.gravity = defaults.gravity;
    }
    if !config.minimum_terminal_speed.is_finite() || config.minimum_terminal_speed <= 0.0 {
        config.minimum_terminal_speed = defaults.minimum_terminal_speed;
    }
    if !config.maximum_terminal_speed.is_finite()
        || config.maximum_terminal_speed < config.minimum_terminal_speed
    {
        config.maximum_terminal_speed = config
            .minimum_terminal_speed
            .max(defaults.maximum_terminal_speed);
    }
    if !config.wind_strength.is_finite() || config.wind_strength < 0.0 {
        config.wind_strength = defaults.wind_strength;
    }
    if !config.wind_direction_x.is_finite() {
        config.wind_direction_x = defaults.wind_direction_x;
    }
    if !config.wind_direction_y.is_finite() {
        config.wind_direction_y = defaults.wind_direction_y;
    }
    let direction_length = config
        .wind_direction_x
        .hypot(config.wind_direction_y)
        .max(f32::EPSILON);
    config.wind_direction_x /= direction_length;
    config.wind_direction_y /= direction_length;
    if !config.turbulence.is_finite() || config.turbulence < 0.0 {
        config.turbulence = defaults.turbulence;
    }
    config
}

#[cfg(test)]
mod tests {
    use super::{
        BlockedRegion, BrushStroke, CollisionSurface, ParticleSystem, PhysicsConfig,
        SurfaceOrientation, ACCUMULATION_BINS, INITIAL_DOCK_SNOW_DEPTH,
    };

    #[test]
    fn gravity_accelerates_particles_toward_variable_terminal_speeds() {
        let mut system = ParticleSystem::new(64, 7, PhysicsConfig::default());
        let initial_state: Vec<_> = system
            .particles()
            .iter()
            .map(|particle| (particle.y, particle.velocity_y))
            .collect();

        for _ in 0..120 {
            system.update(1.0 / 120.0);
        }

        assert!(system
            .particles()
            .iter()
            .zip(initial_state)
            .filter(|(particle, (initial_y, _))| particle.y >= *initial_y)
            .all(|(particle, (_, initial_velocity))| particle.velocity_y >= initial_velocity));
        assert!(system.particles().iter().all(|particle| {
            particle.velocity_y <= particle.terminal_speed
                && particle.terminal_speed >= PhysicsConfig::default().minimum_terminal_speed
                && particle.terminal_speed <= PhysicsConfig::default().maximum_terminal_speed
        }));
        assert!(system
            .particles()
            .iter()
            .any(|particle| particle.velocity_y == particle.terminal_speed));
    }

    #[test]
    fn particles_recycle_without_changing_pool_size() {
        let mut system = ParticleSystem::new(24, 11, PhysicsConfig::default());

        for _ in 0..6_000 {
            system.update(1.0 / 120.0);
        }

        assert_eq!(system.particles().len(), 24);
        assert!(system
            .particles()
            .iter()
            .all(|particle| (-0.02..=1.02).contains(&particle.y)));
    }

    #[test]
    fn particles_are_distributed_across_the_full_screen_width() {
        let system = ParticleSystem::new(4_096, 13, PhysicsConfig::default());
        let mut columns = [0; 8];
        for particle in system.particles() {
            let column = ((particle.x * columns.len() as f32) as usize).min(columns.len() - 1);
            columns[column] += 1;
            assert!((0.0..=1.0).contains(&particle.x));
            assert!((-0.02..=1.02).contains(&particle.y));
        }

        assert!(columns.iter().all(|count| *count > 400), "{columns:?}");
    }

    #[test]
    fn wind_and_turbulence_create_horizontal_drift() {
        let mut system = ParticleSystem::new(
            32,
            19,
            PhysicsConfig {
                wind_strength: 0.08,
                turbulence: 0.06,
                ..PhysicsConfig::default()
            },
        );
        let initial_positions: Vec<_> = system.particles().iter().map(|p| p.x).collect();

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        assert!(system
            .particles()
            .iter()
            .zip(initial_positions)
            .any(|(particle, initial_x)| (particle.x - initial_x).abs() > 0.001));
    }

    #[test]
    fn wind_keeps_the_selected_direction_unless_it_alternates() {
        let mut steady = ParticleSystem::new(
            1,
            71,
            PhysicsConfig {
                wind_strength: 0.05,
                wind_direction_x: 1.0,
                wind_direction_y: 0.0,
                turbulence: 0.0,
                wind_alternates: false,
                ..PhysicsConfig::default()
            },
        );
        for _ in 0..600 {
            steady.update(1.0 / 120.0);
        }
        assert!(
            steady.particles()[0].velocity_x > 0.0,
            "selected rightward wind must not reverse: {}",
            steady.particles()[0].velocity_x
        );

        let mut alternating = ParticleSystem::new(
            1,
            73,
            PhysicsConfig {
                wind_strength: 0.08,
                wind_direction_x: 1.0,
                wind_direction_y: 0.0,
                turbulence: 0.0,
                wind_alternates: true,
                ..PhysicsConfig::default()
            },
        );
        alternating.particles[0].wind_influence = 1.0;
        let mut saw_positive = false;
        let mut saw_negative = false;
        for _ in 0..1200 {
            alternating.update(1.0 / 120.0);
            let velocity_x = alternating.particles()[0].velocity_x;
            saw_positive |= velocity_x > 0.0;
            saw_negative |= velocity_x < 0.0;
        }
        assert!(
            saw_positive && saw_negative,
            "blizzard wind should swing both ways"
        );
    }

    #[test]
    fn invalid_time_does_not_change_the_simulation() {
        let mut system = ParticleSystem::new(8, 23, PhysicsConfig::default());
        let initial = system.particles().to_vec();

        system.update(f32::NAN);
        system.update(-1.0);
        system.update(f32::INFINITY);

        assert_eq!(system.particles(), initial);
    }

    #[test]
    fn particles_collide_and_accumulate_on_window_edges() {
        let mut system = ParticleSystem::new(1, 29, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 7,
            x: 0.4,
            y: 0.5,
            width: 0.2,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        let particle = &mut system.particles[0];
        particle.x = 0.5;
        particle.y = 0.49;
        particle.velocity_y = 0.5;
        particle.terminal_speed = 1.0;

        system.update(0.05);

        let snow = system.snow_surfaces().first().unwrap();
        assert!(snow.snow_depths.iter().any(|depth| *depth > 0.0));
        assert!(
            snow.snow_depths
                .iter()
                .filter(|depth| **depth > 0.0)
                .count()
                >= 4
        );
        let peak = snow
            .snow_depths
            .iter()
            .enumerate()
            .max_by(|(_, first), (_, second)| first.total_cmp(second))
            .map(|(index, _)| index)
            .unwrap();
        assert!(snow.snow_depths[peak] > snow.snow_depths[peak - 1]);
        assert!(snow.snow_depths[peak - 1] > snow.snow_depths[peak - 2]);
        assert_eq!(system.particles().len(), 1);
        assert_eq!(system.particles()[0].y, 0.0);
    }

    #[test]
    fn window_edge_margin_prevents_hidden_edge_deposits() {
        let mut system = ParticleSystem::new(1, 30, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 8,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.08,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.particles[0].x = 0.15;
        system.particles[0].y = 0.49;
        system.particles[0].velocity_y = 0.5;
        system.particles[0].terminal_speed = 1.0;

        system.update(0.05);

        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == 0.0));
    }

    #[test]
    fn surface_updates_keep_snow_attached_and_remove_closed_windows() {
        let mut system = ParticleSystem::new(1, 31, PhysicsConfig::default());
        let surface = CollisionSurface {
            id: 42,
            x: 0.1,
            y: 0.3,
            width: 0.6,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        system.set_surfaces(&[surface]);
        system.snow_surfaces[0].snow_depths[3] = 0.02;
        let particles_before_move = system.particles().to_vec();
        system.set_surfaces(&[CollisionSurface { y: 0.35, ..surface }]);

        assert_eq!(system.snow_surfaces()[0].y, 0.35);
        assert_eq!(system.snow_surfaces()[0].snow_depths[3], 0.02);
        assert_eq!(system.particles(), particles_before_move);
        system.set_surfaces(&[]);
        assert!(system.snow_surfaces().is_empty());
        assert_eq!(system.particles(), particles_before_move);
    }

    #[test]
    fn moving_a_snowy_surface_keeps_its_profile_at_the_new_position() {
        let mut system = ParticleSystem::new(1, 31, PhysicsConfig::default());
        let surface = CollisionSurface {
            id: 21,
            x: 0.1,
            y: 0.3,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        system.set_surfaces(&[surface]);
        system.snow_surfaces[0].snow_depths[0] = 0.02;

        system.set_surfaces(&[CollisionSurface {
            x: 0.15,
            y: 0.45,
            ..surface
        }]);

        assert_eq!(system.snow_surfaces()[0].x, 0.15);
        assert_eq!(system.snow_surfaces()[0].y, 0.45);
        assert_eq!(system.snow_surfaces()[0].snow_depths[0], 0.02);
    }

    #[test]
    fn subpixel_surface_motion_keeps_snow_attached_without_emitting_particles() {
        let surface = CollisionSurface {
            id: 21,
            x: 0.4,
            y: 0.35,
            width: 0.3,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        let mut system = ParticleSystem::new(4, 17, PhysicsConfig::default());
        system.set_surfaces(&[surface]);
        system.snow_surfaces[0].snow_depths[10] = 0.02;
        let particles_before_move = system.particles().to_vec();

        system.set_surfaces(&[CollisionSurface {
            x: surface.x + 0.0003,
            ..surface
        }]);

        assert_eq!(system.snow_surfaces()[0].snow_depths[10], 0.02);
        assert_eq!(system.particles(), particles_before_move);
    }

    #[test]
    fn accumulation_speed_scales_snow_deposition() {
        let surface = CollisionSurface {
            id: 22,
            x: 0.0,
            y: 0.5,
            width: 1.0,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        let mut normal = ParticleSystem::new(1, 23, PhysicsConfig::default());
        let mut fast = ParticleSystem::new(1, 23, PhysicsConfig::default());
        normal.set_accumulation_speed(0.25);
        for system in [&mut normal, &mut fast] {
            system.set_surfaces(&[surface]);
            system.particles[0].x = 0.5;
            system.particles[0].y = 0.49;
            system.particles[0].velocity_y = 0.5;
            system.particles[0].terminal_speed = 1.0;
        }

        fast.set_accumulation_speed(0.75);

        normal.update(0.05);
        fast.update(0.05);

        assert!(
            fast.snow_surfaces()[0].snow_depths[24] > normal.snow_surfaces()[0].snow_depths[24]
        );
    }

    #[test]
    fn default_particle_count_reaches_a_full_drift_in_about_five_minutes() {
        let mut system = ParticleSystem::new(
            9_000,
            89,
            PhysicsConfig {
                wind_strength: 0.0,
                turbulence: 0.0,
                ..PhysicsConfig::default()
            },
        );
        system.set_surfaces(&[CollisionSurface {
            id: 25,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);

        for step in 0..6_000 {
            system.update(0.05);
            if step == 4_799 {
                let depths = &system.snow_surfaces()[0].snow_depths;
                let average_depth = depths.iter().sum::<f32>() / depths.len() as f32;
                assert!(
                    average_depth < 0.028,
                    "drift filled too early at 240 seconds: {average_depth}"
                );
            }
        }

        let depths = &system.snow_surfaces()[0].snow_depths;
        let average_depth = depths.iter().sum::<f32>() / depths.len() as f32;
        assert!(
            average_depth >= 0.028,
            "expected near-full drift at 300 seconds, got {average_depth}"
        );
        assert!(depths.iter().all(|depth| *depth <= 0.03));
    }

    #[test]
    fn zero_accumulation_speed_preserves_snow_and_does_not_intercept_particles() {
        let surface = CollisionSurface {
            id: 22,
            x: 0.4,
            y: 0.5,
            width: 0.2,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: true,
            maximum_depth: 0.03,
        };
        let mut disabled = ParticleSystem::new(1, 23, PhysicsConfig::default());
        disabled.set_accumulation_speed(0.0);
        disabled.set_surfaces(&[surface]);
        let mut control = ParticleSystem::new(1, 23, PhysicsConfig::default());
        for system in [&mut disabled, &mut control] {
            let particle = &mut system.particles[0];
            particle.x = 0.5;
            particle.y = 0.49;
            particle.velocity_y = 0.5;
            particle.terminal_speed = 1.0;
        }
        disabled.snow_surfaces[0].snow_depths[24] = 0.02;
        let snow_before = disabled.snow_surfaces[0].snow_depths.clone();

        disabled.update(0.05);
        control.update(0.05);

        assert_eq!(disabled.particles(), control.particles());
        assert_eq!(disabled.snow_surfaces()[0].snow_depths, snow_before);
    }

    #[test]
    fn zero_accumulation_speed_does_not_seed_persistent_dock_snow() {
        let mut system = ParticleSystem::new(0, 23, PhysicsConfig::default());
        system.set_accumulation_speed(0.0);
        system.set_surfaces(&[CollisionSurface {
            id: 24,
            x: 0.2,
            y: 0.5,
            width: 0.6,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: true,
            maximum_depth: 0.03,
        }]);

        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == 0.0));

        system.update(0.05);
        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == 0.0));
    }

    #[test]
    fn window_wind_drafts_affect_only_a_few_nearby_particles() {
        let mut system = ParticleSystem::new(
            8,
            77,
            PhysicsConfig {
                wind_strength: 0.0,
                turbulence: 0.0,
                ..PhysicsConfig::default()
            },
        );
        let region = BlockedRegion {
            id: 51,
            x: 0.4,
            y: 0.2,
            width: 0.2,
            height: 0.6,
        };
        system.set_wind_draft_regions(&[region]);
        assert!((8.0..=25.0).contains(&system.draft_windows[0].next_gust_at));
        system.set_window_wind_draft_enabled(true);
        for (index, particle) in system.particles.iter_mut().enumerate() {
            particle.x = if index % 2 == 0 { 0.39 } else { 0.61 };
            particle.y = 0.5;
        }

        for _ in 0..20 {
            system.draft_windows[0].next_gust_at = system.elapsed;
            system.update_window_wind_drafts();
            if system
                .particles
                .iter()
                .any(|particle| particle.window_draft)
            {
                break;
            }
        }

        let affected: Vec<_> = system
            .particles
            .iter()
            .enumerate()
            .filter(|(_, particle)| particle.window_draft)
            .map(|(index, particle)| (index, *particle))
            .collect();
        assert!(!affected.is_empty());
        assert!(affected.len() <= 3);
        for (_, particle) in &affected {
            if particle.x < region.x {
                assert!(particle.draft_velocity_x > 0.0);
            } else {
                assert!(particle.draft_velocity_x < 0.0);
            }
            assert!(particle.draft_velocity_x.abs() <= 0.034);
            assert!(particle.draft_velocity_y.abs() <= 0.006);
        }

        let (particle_index, particle) = affected[0];
        let direction = particle.draft_velocity_x.signum();
        for _ in 0..30 {
            system.update(0.05);
        }
        let particle = &system.particles()[particle_index];
        assert!(particle.window_draft_entered);
        assert!(particle.x > region.x && particle.x < region.x + region.width);
        assert_eq!(direction, particle.draft_velocity_x.signum());
    }

    #[test]
    fn disabling_window_wind_draft_cancels_transient_impulses() {
        let mut system = ParticleSystem::new(1, 79, PhysicsConfig::default());
        system.set_window_wind_draft_enabled(true);
        let particle = &mut system.particles[0];
        particle.window_draft = true;
        particle.window_draft_entered = true;
        particle.draft_window_id = 1;
        particle.draft_velocity_x = 0.03;
        particle.draft_velocity_y = -0.004;

        system.set_window_wind_draft_enabled(false);

        let particle = &system.particles()[0];
        assert!(!particle.window_draft);
        assert!(!particle.window_draft_entered);
        assert_eq!(particle.draft_window_id, 0);
        assert_eq!(particle.draft_velocity_x, 0.0);
        assert_eq!(particle.draft_velocity_y, 0.0);
    }

    #[test]
    fn disabled_window_wind_draft_never_generates_a_gust() {
        let mut system = ParticleSystem::new(1, 83, PhysicsConfig::default());
        system.set_wind_draft_regions(&[BlockedRegion {
            id: 52,
            x: 0.4,
            y: 0.2,
            width: 0.2,
            height: 0.6,
        }]);
        system.draft_windows[0].next_gust_at = 0.0;
        system.particles[0].x = 0.39;
        system.particles[0].y = 0.5;

        system.update_window_wind_drafts();

        assert!(!system.particles()[0].window_draft);
        assert_eq!(system.particles()[0].draft_velocity_x, 0.0);
    }

    #[test]
    fn particle_count_can_be_changed_within_the_supported_pool() {
        let mut system = ParticleSystem::new(2, 29, PhysicsConfig::default());
        system.set_particle_count(500);
        assert_eq!(system.particles().len(), 500);
        assert!(system.particles()[2..]
            .iter()
            .all(|particle| particle.y == 0.0));
        system.set_particle_count(30_000);
        assert_eq!(system.particles().len(), 20_000);
    }

    #[test]
    fn clearing_snow_removes_all_surface_accumulation() {
        let mut system = ParticleSystem::new(1, 33, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 22,
            x: 0.1,
            y: 0.3,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths.fill(0.02);

        system.clear_snow();

        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == 0.0));
    }

    #[test]
    fn cursor_brush_clears_snow_only_near_its_path() {
        let mut system = ParticleSystem::new(1, 43, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 9,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths.fill(0.02);

        system.brush_snow_at(0.5, 0.48, 0.04, 0.04, 1.0);

        let depths = &system.snow_surfaces()[0].snow_depths;
        assert_eq!(depths[23], 0.0);
        assert_eq!(depths[0], 0.02);
        assert_eq!(depths[47], 0.02);
    }

    #[test]
    fn mitten_pushes_accumulated_snow_without_destroying_its_mass() {
        let mut system = ParticleSystem::new(1, 47, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 10,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths.fill(0.02);
        // Isolate the redistribution behaviour from snowball creation.
        system.next_snowball_spawn_at = f32::MAX;
        let total_depth_before: f32 = system.snow_surfaces()[0].snow_depths.iter().sum();

        system.push_snow_at(BrushStroke {
            x: 0.5,
            y: 0.48,
            radius_x: 0.08,
            radius_y: 0.04,
            strength: 1.0,
            direction_x: 1.0,
            direction_y: 0.0,
        });

        let depths = &system.snow_surfaces()[0].snow_depths;
        let total_depth_after: f32 = depths.iter().sum();
        assert!((total_depth_after - total_depth_before).abs() < 0.000001);
        assert!(depths[23] < 0.02);
        assert!(depths[24..27].iter().any(|depth| *depth > 0.02));
        assert!(depths[24..].iter().sum::<f32>() > depths[..23].iter().sum::<f32>());
        assert_eq!(system.snow_surfaces()[0].snow_depths[0], 0.02);
    }

    #[test]
    fn mitten_pushes_particles_in_stroke_direction_without_deleting_them() {
        let mut system = ParticleSystem::new(2, 48, PhysicsConfig::default());
        system.particles[0].x = 0.5;
        system.particles[0].y = 0.5;
        system.particles[0].velocity_x = 0.0;
        system.particles[0].velocity_y = 0.02;
        let initial_count = system.particles().len();

        system.push_snow_at(BrushStroke {
            x: 0.45,
            y: 0.5,
            radius_x: 0.1,
            radius_y: 0.1,
            strength: 1.0,
            direction_x: 1.0,
            direction_y: 0.0,
        });

        assert!(system.particles()[0].velocity_x > 0.0);
        assert_eq!(system.particles().len(), initial_count);
        system.update(0.05);
        assert!(system.particles()[0].x > 0.5);
        assert_eq!(system.particles().len(), initial_count);
    }

    #[test]
    fn surface_updates_reject_invalid_geometry_and_limit_capacity() {
        let mut system = ParticleSystem::new(0, 37, PhysicsConfig::default());
        let surfaces: Vec<_> = (0..(super::MAX_ACCUMULATION_SURFACES + 1))
            .map(|id| CollisionSurface {
                id: id as u64,
                x: 0.0,
                y: 0.2,
                width: 1.0,
                edge_margin: 0.0,
                orientation: SurfaceOrientation::Horizontal,
                persistent: false,
                maximum_depth: 0.03,
            })
            .chain([CollisionSurface {
                id: 1000,
                x: f32::NAN,
                y: 0.2,
                width: 1.0,
                edge_margin: 0.0,
                orientation: SurfaceOrientation::Horizontal,
                persistent: false,
                maximum_depth: 0.03,
            }])
            .collect();
        system.set_surfaces(&surfaces);

        assert_eq!(
            system.snow_surfaces().len(),
            super::MAX_ACCUMULATION_SURFACES
        );
        assert_eq!(
            system.snow_surfaces()[0].snow_depths.len(),
            ACCUMULATION_BINS
        );
    }

    #[test]
    fn accumulated_snow_slowly_melts() {
        let mut system = ParticleSystem::new(0, 41, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 8,
            x: 0.1,
            y: 0.3,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths[0] = 0.02;

        system.update(0.05);

        assert!(system.snow_surfaces()[0].snow_depths[0] < 0.02);
        assert!(system.snow_surfaces()[0].snow_depths[0] > 0.0);
    }

    #[test]
    fn accumulated_snow_relaxes_into_a_smooth_ridge_quickly() {
        let mut system = ParticleSystem::new(0, 42, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 8,
            x: 0.1,
            y: 0.3,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths[24] = 0.02;

        for _ in 0..30 {
            system.update(1.0 / 120.0);
        }

        let depths = &system.snow_surfaces()[0].snow_depths;
        assert!(depths[23] > 0.0 && depths[25] > 0.0);
        assert!(depths[23] > depths[22] && depths[25] > depths[26]);
        assert!(
            depths
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 0.001),
            "{depths:?}"
        );
    }

    #[test]
    fn snow_accumulation_is_irregular_and_stops_at_its_height_limit() {
        let mut system = ParticleSystem::new(
            ACCUMULATION_BINS,
            47,
            PhysicsConfig {
                wind_strength: 0.0,
                turbulence: 0.0,
                ..PhysicsConfig::default()
            },
        );
        system.set_surfaces(&[CollisionSurface {
            id: 9,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.01,
        }]);
        for _ in 0..1 {
            for (index, particle) in system.particles.iter_mut().enumerate() {
                particle.x = 0.1 + (index as f32 + 0.5) * 0.8 / ACCUMULATION_BINS as f32;
                particle.y = 0.49;
                particle.velocity_x = 0.0;
                particle.velocity_y = 0.5;
                particle.terminal_speed = 1.0;
            }
            system.update(0.05);
        }

        let depths = &system.snow_surfaces()[0].snow_depths;
        assert!(depths.iter().all(|depth| *depth <= 0.01));
        assert!(depths.iter().filter(|depth| **depth > 0.0).count() > 1);
        assert!(depths.windows(2).any(|pair| pair[0] != pair[1]));
        let varied_bin = 0.1 + 0.5 * 0.8 / ACCUMULATION_BINS as f32;

        for _ in 0..4_000 {
            system.particles[0].x = varied_bin;
            system.particles[0].y = 0.49;
            system.particles[0].velocity_x = 0.0;
            system.particles[0].velocity_y = 0.5;
            system.particles[0].terminal_speed = 1.0;
            system.update(0.05);
        }
        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth <= 0.01));
    }

    #[test]
    fn snow_can_land_on_a_launchpad_surface_at_the_top_of_the_screen() {
        let mut system = ParticleSystem::new(1, 53, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 10,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.particles[0].x = 0.5;
        system.particles[0].y = -0.01;
        system.particles[0].velocity_y = 0.5;
        system.particles[0].terminal_speed = 1.0;

        system.update(0.05);

        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .any(|depth| *depth > 0.0));
    }

    #[test]
    fn dock_side_surface_accumulates_snow_when_particles_cross_its_face() {
        let mut system = ParticleSystem::new(1, 59, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 11,
            x: 0.4,
            y: 0.1,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::VerticalToRight,
            persistent: true,
            maximum_depth: 0.03,
        }]);
        system.particles[0].x = 0.401;
        system.particles[0].y = 0.5;
        system.particles[0].velocity_x = -0.09;
        system.particles[0].velocity_y = 0.0;

        system.update(0.05);

        let snow = &system.snow_surfaces()[0];
        assert!(snow
            .snow_depths
            .iter()
            .any(|depth| *depth > INITIAL_DOCK_SNOW_DEPTH));
        assert!(
            snow.snow_depths
                .iter()
                .filter(|depth| **depth > 0.0)
                .count()
                >= 4
        );
    }

    #[test]
    fn persistent_dock_surfaces_start_and_remain_snowy_until_removed() {
        let mut system = ParticleSystem::new(0, 61, PhysicsConfig::default());
        let dock = CollisionSurface {
            id: 12,
            x: 0.2,
            y: 0.5,
            width: 0.6,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: true,
            maximum_depth: 0.03,
        };
        system.set_surfaces(&[dock]);

        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == INITIAL_DOCK_SNOW_DEPTH));

        for _ in 0..1200 {
            system.update(1.0 / 120.0);
        }
        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth >= INITIAL_DOCK_SNOW_DEPTH));

        system.clear_snow();
        system.set_surfaces(&[dock]);
        system.update(0.05);
        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == 0.0));

        system.set_surfaces(&[]);
        assert!(system.snow_surfaces().is_empty());
        system.set_surfaces(&[dock]);
        assert!(system.snow_surfaces()[0]
            .snow_depths
            .iter()
            .all(|depth| *depth == INITIAL_DOCK_SNOW_DEPTH));
    }
}
pub const MAX_ACCUMULATION_SURFACES: usize = 64;
pub const ACCUMULATION_BINS: usize = 48;
const SNOW_DEPOSIT_DEPTH: f32 = 0.0000065;
const INITIAL_DOCK_SNOW_DEPTH: f32 = 0.002;
const MAX_SNOW_DEPTH: f32 = 0.035;
const SNOW_MELT_RATE: f32 = 0.00002;
/// Surfaces whose widths differ by less than this are treated as the same shape
/// that merely moved, so their snow profile is copied bin for bin.
const SAME_WIDTH_TOLERANCE: f32 = 0.00005;
/// Horizontal edges at the same height (within this) belong to the same "row" of a window top.
const SAME_ROW_TOLERANCE: f32 = 0.002;
/// Tangent of the angle of repose (about 35 degrees) used to slump steep piles.
const REPOSE_TANGENT: f32 = 0.7;
const SLUMP_RATE: f32 = 0.5;
const SLUMP_PASSES: usize = 2;
/// Accumulated depth (summed over bins) that turns into one falling flake when
/// the mitten sweeps snow past the end of a surface.
const SPILL_FLAKE_VOLUME: f32 = 0.002;
const MAX_SPILL_FLAKES_PER_STROKE: usize = 4;
const DEFAULT_DISPLAY_ASPECT: f32 = 1.6;
/// Snow packed by the mitten is allowed to grow past the untouched maximum
/// depth, but only snow moved by the tool can reach this ceiling.
const SCULPTED_SNOW_DEPTH_LIMIT: f32 = 0.09;
const SNOWBALL_RADIUS: f32 = 0.012;
const SNOWBALL_GRAVITY: f32 = 0.42;
const SNOWBALL_MAX_SPEED: f32 = 1.6;
const SNOWBALL_BREAK_SPEED: f32 = 0.42;
const SNOWBALL_FRICTION: f32 = 1.4;
const SNOWBALL_REST_SPEED: f32 = 0.003;
const SNOWBALL_SPAWN_INTERVAL: f32 = 0.22;
const SNOWBALL_CONTACT_PASSES: usize = 4;
const SNOWBALL_CONTACT_SLOP: f32 = 0.0002;
const SNOWBALL_SNOW_COST: f32 = 0.0035;
const SNOWBALL_BREAK_DEPTH: f32 = 0.004;
const SNOWBALL_SPAWN_MIN_DEPTH: f32 = 0.004;
const MAX_SNOWBALLS: usize = 32;
const MAX_SNOWMEN: usize = 8;
const MAX_SNOWBALL_OBSTACLES: usize = 64;
const SNOWBALL_HIT_RADIUS_SCALE: f32 = 1.35;

#[derive(Debug, Clone, Copy, PartialEq)]
/// Collision geometry consumed by the shared particle and accumulation system.
pub struct CollisionSurface {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub edge_margin: f32,
    pub orientation: SurfaceOrientation,
    pub persistent: bool,
    pub maximum_depth: f32,
}

#[cfg(test)]
mod accumulation_tests {
    use super::*;

    fn flat_surface(id: u64) -> SnowSurface {
        SnowSurface {
            id,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
            snow_depths: vec![0.0; ACCUMULATION_BINS],
        }
    }

    #[test]
    fn steep_piles_slump_to_the_angle_of_repose_without_losing_snow() {
        let mut surface = flat_surface(1);
        surface.snow_depths[20..28].fill(0.03);
        let before: f32 = surface.snow_depths.iter().sum();

        for _ in 0..200 {
            slump_snow(&mut surface, 1.6);
        }

        let after: f32 = surface.snow_depths.iter().sum();
        let maximum_step = REPOSE_TANGENT * (0.8 / ACCUMULATION_BINS as f32) * 1.6;
        assert!((after - before).abs() < 0.00001, "{before} -> {after}");
        assert!(surface
            .snow_depths
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() <= maximum_step + 0.0001));
        assert!(surface
            .snow_depths
            .iter()
            .all(|depth| (0.0..=0.03).contains(depth)));
        // The pile spread out instead of staying a cliff.
        assert!(surface.snow_depths[19] > 0.0 && surface.snow_depths[28] > 0.0);
    }

    #[test]
    fn gentle_snow_is_left_alone_by_slumping() {
        let mut surface = flat_surface(1);
        for (bin, depth) in surface.snow_depths.iter_mut().enumerate() {
            *depth = 0.01 + bin as f32 * 0.00005;
        }
        let before = surface.snow_depths.clone();
        slump_snow(&mut surface, 1.6);
        assert_eq!(surface.snow_depths, before);
    }

    #[test]
    fn display_aspect_ignores_invalid_values() {
        let mut system = ParticleSystem::new(1, 5, PhysicsConfig::default());
        system.set_display_aspect(f32::NAN);
        system.set_display_aspect(-2.0);
        assert_eq!(system.display_aspect, DEFAULT_DISPLAY_ASPECT);
        system.set_display_aspect(2.0);
        assert_eq!(system.display_aspect, 2.0);
        system.set_display_aspect(100.0);
        assert_eq!(system.display_aspect, 4.0);
    }

    #[test]
    fn covering_part_of_a_window_edge_keeps_the_snow_where_it_lies() {
        let owner = 7_u64 << 32;
        let whole = CollisionSurface {
            id: owner,
            x: 0.2,
            y: 0.5,
            width: 0.6,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        let mut system = ParticleSystem::new(1, 9, PhysicsConfig::default());
        system.set_surfaces(&[whole]);
        let half = ACCUMULATION_BINS / 2;
        system.snow_surfaces[0].snow_depths[..half].fill(0.02);
        system.snow_surfaces[0].snow_depths[half..].fill(0.01);

        // Another window now covers the middle: two visible segments remain.
        system.set_surfaces(&[
            CollisionSurface {
                width: 0.3,
                ..whole
            },
            CollisionSurface {
                id: owner | 1,
                x: 0.6,
                width: 0.2,
                ..whole
            },
        ]);

        let surfaces = system.snow_surfaces();
        assert_eq!(surfaces.len(), 2);
        let left = &surfaces[0].snow_depths;
        let right = &surfaces[1].snow_depths;
        assert!((left[0] - 0.02).abs() < 0.00001, "{left:?}");
        assert!(left.iter().all(|depth| *depth >= 0.0169), "{left:?}");
        assert!(
            right.iter().all(|depth| (depth - 0.01).abs() < 0.00001),
            "{right:?}"
        );
    }

    #[test]
    fn dragging_a_window_still_carries_its_snow_along() {
        let owner = 9_u64 << 32;
        let surface = CollisionSurface {
            id: owner,
            x: 0.2,
            y: 0.5,
            width: 0.4,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        let mut system = ParticleSystem::new(1, 11, PhysicsConfig::default());
        system.set_surfaces(&[surface]);
        system.snow_surfaces[0].snow_depths[5] = 0.02;

        // Moved and resized at once: neither edge stays put, so the profile is carried over.
        system.set_surfaces(&[CollisionSurface {
            x: 0.4,
            width: 0.3,
            ..surface
        }]);

        assert_eq!(system.snow_surfaces()[0].snow_depths[5], 0.02);
    }

    #[test]
    fn snow_from_unrelated_windows_is_not_inherited() {
        let first = CollisionSurface {
            id: 3_u64 << 32,
            x: 0.2,
            y: 0.5,
            width: 0.4,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        };
        let mut system = ParticleSystem::new(1, 13, PhysicsConfig::default());
        system.set_surfaces(&[first]);
        system.snow_surfaces[0].snow_depths.fill(0.02);

        system.set_surfaces(&[
            first,
            CollisionSurface {
                id: 4_u64 << 32,
                x: 0.3,
                width: 0.2,
                ..first
            },
        ]);

        assert!(system.snow_surfaces()[1]
            .snow_depths
            .iter()
            .all(|depth| *depth == 0.0));
    }

    #[test]
    fn snow_swept_past_a_window_end_falls_as_flakes() {
        let mut system = ParticleSystem::new(64, 47, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 10,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths.fill(0.02);
        let before: f32 = system.snow_surfaces()[0].snow_depths.iter().sum();
        let particle_count = system.particles().len();

        system.push_snow_at(BrushStroke {
            x: 0.88,
            y: 0.48,
            radius_x: 0.08,
            radius_y: 0.04,
            strength: 1.0,
            direction_x: 1.0,
            direction_y: 0.0,
        });

        let after: f32 = system.snow_surfaces()[0].snow_depths.iter().sum();
        assert!(after < before - 0.005, "{before} -> {after}");
        assert_eq!(system.particles().len(), particle_count);
        // Freshly spilled flakes start just beyond the right end moving outwards;
        // regular flakes never start with such a horizontal speed.
        assert!(system
            .particles()
            .iter()
            .any(|particle| particle.velocity_x >= 0.012 && (particle.x - 0.902).abs() < 0.001));
    }

    #[test]
    fn sweeping_inside_a_window_still_conserves_snow_and_spawns_nothing() {
        let mut system = ParticleSystem::new(64, 48, PhysicsConfig::default());
        system.set_surfaces(&[CollisionSurface {
            id: 10,
            x: 0.1,
            y: 0.5,
            width: 0.8,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth: 0.03,
        }]);
        system.snow_surfaces[0].snow_depths.fill(0.02);
        // Balls are covered by their own tests; this checks only the pushed snow.
        system.next_snowball_spawn_at = f32::MAX;
        let particles_before = system.particles().to_vec();

        system.push_snow_at(BrushStroke {
            x: 0.5,
            y: 0.48,
            radius_x: 0.04,
            radius_y: 0.04,
            strength: 1.0,
            direction_x: 1.0,
            direction_y: 0.0,
        });

        let after: f32 = system.snow_surfaces()[0].snow_depths.iter().sum();
        assert!((after - 0.02 * ACCUMULATION_BINS as f32).abs() < 0.000001);
        assert!(system
            .particles()
            .iter()
            .zip(&particles_before)
            .all(|(now, before)| now.x == before.x && now.y == before.y));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnowSurface {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub edge_margin: f32,
    pub orientation: SurfaceOrientation,
    pub persistent: bool,
    pub maximum_depth: f32,
    pub snow_depths: Vec<f32>,
}

#[cfg(test)]
mod snowball_tests {
    use super::*;

    fn flat_surface(id: u64, x: f32, y: f32, width: f32, maximum_depth: f32) -> CollisionSurface {
        CollisionSurface {
            id,
            x,
            y,
            width,
            edge_margin: 0.0,
            orientation: SurfaceOrientation::Horizontal,
            persistent: false,
            maximum_depth,
        }
    }

    fn free_ball(id: u64, x: f32, y: f32) -> Snowball {
        Snowball {
            id,
            x,
            y,
            velocity_x: 0.0,
            velocity_y: 0.0,
            radius: SNOWBALL_RADIUS,
            state: SnowballState::Free,
            group: 0,
            surface_id: NO_SURFACE_ID,
            surface_offset_x: 0.0,
            roll: 0.0,
            placed: false,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
        }
    }

    #[test]
    fn mitten_builds_snow_above_the_normal_limit() {
        let mut system = ParticleSystem::new(0, 5, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.5, 0.8, 0.02)]);
        system.snow_surfaces[0].snow_depths.fill(0.02);
        system.next_snowball_spawn_at = f32::MAX;

        for _ in 0..24 {
            system.push_snow_at(BrushStroke {
                x: 0.48,
                y: 0.48,
                radius_x: 0.06,
                radius_y: 0.04,
                strength: 1.0,
                direction_x: 1.0,
                direction_y: 0.0,
            });
        }

        let highest = system.snow_surfaces[0]
            .snow_depths
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        assert!(
            highest > 0.02,
            "the mitten must be able to pile snow past the untouched limit, got {highest}"
        );
        assert!(highest <= SCULPTED_SNOW_DEPTH_LIMIT + f32::EPSILON);
    }

    #[test]
    fn repeated_mitten_strokes_build_a_taller_drift() {
        let mut system = ParticleSystem::new(0, 43, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.5, 0.8, 0.02)]);
        system.snow_surfaces[0].snow_depths.fill(0.02);
        system.next_snowball_spawn_at = f32::MAX;

        for _ in 0..120 {
            system.push_snow_at(BrushStroke {
                x: 0.82,
                y: 0.48,
                radius_x: 0.05,
                radius_y: 0.04,
                strength: 1.0,
                direction_x: -1.0,
                direction_y: 0.0,
            });
            system.update(1.0 / 120.0);
        }

        let highest = system.snow_surfaces[0]
            .snow_depths
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        assert!(
            highest > 0.03,
            "repeated strokes should build a visibly taller drift, got {highest}"
        );
    }

    #[test]
    fn fresh_snow_does_not_layer_on_a_sculpted_pile() {
        let mut system = ParticleSystem::new(1, 7, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.5, 0.8, 0.02)]);
        // A fully sculpted row is above the untouched limit everywhere, so no
        // landing flake is allowed to add anything to it.
        system.snow_surfaces[0].snow_depths.fill(0.05);
        let total_before: f32 = system.snow_surfaces[0].snow_depths.iter().sum();
        let particle = &mut system.particles[0];
        particle.x = 0.5;
        particle.y = 0.45;
        particle.velocity_x = 0.0;
        particle.velocity_y = 0.5;
        particle.terminal_speed = 1.0;

        system.update(0.05);

        let total_after: f32 = system.snow_surfaces[0].snow_depths.iter().sum();
        assert!(
            total_after <= total_before + 0.000001,
            "new snowfall must not add mass to a sculpted drift: {total_before} -> {total_after}"
        );
    }

    #[test]
    fn explicit_roll_gesture_creates_a_small_ball() {
        let mut system = ParticleSystem::new(0, 11, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.5, 0.8, 0.03)]);
        system.snow_surfaces[0].snow_depths.fill(0.02);
        let before: f32 = system.snow_surfaces[0].snow_depths.iter().sum();

        system.push_snow_at(BrushStroke {
            x: 0.5,
            y: 0.48,
            radius_x: 0.05,
            radius_y: 0.04,
            strength: 1.0,
            direction_x: 1.0,
            direction_y: 0.0,
        });
        assert!(
            system.snowballs().is_empty(),
            "an ordinary mitten sweep must not spawn a snowball"
        );

        assert!(system.roll_snowball_at(0.5, 0.48, 1.0));
        assert_eq!(system.snowballs().len(), 1);
        let after: f32 = system.snow_surfaces[0].snow_depths.iter().sum();
        assert!(after < before);
        let ball = system.snowballs()[0];
        assert!(ball.y + ball.radius <= 0.5 + f32::EPSILON);
        let start_x = ball.x;
        for _ in 0..60 {
            system.update(1.0 / 120.0);
        }
        assert!(
            (system.snowballs()[0].x - start_x).abs() > 0.005,
            "the spawned ball should keep rolling instead of jittering in place"
        );
    }

    #[test]
    fn snowballs_can_be_grabbed_dragged_and_released() {
        let mut system = ParticleSystem::new(0, 13, PhysicsConfig::default());
        system.snowballs.push(free_ball(1, 0.5, 0.5));

        let id = system
            .grab_snowball(0.5, 0.5)
            .expect("ball should be grabbed");
        assert_eq!(system.snowballs()[0].state, SnowballState::Held);
        assert!(system.drag_snowball(id, 0.6, 0.6, 1.0 / 60.0));
        assert!((system.snowballs()[0].x - 0.6).abs() < f32::EPSILON);
        assert!(system.release_snowball(id));
        assert_eq!(system.snowballs()[0].state, SnowballState::Free);

        let before = system.snowballs()[0].y;
        system.update(1.0 / 60.0);
        assert!(system.snowballs()[0].y > before);
    }

    #[test]
    fn balls_land_on_snow_and_rest_above_it() {
        let mut system = ParticleSystem::new(0, 17, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.5, 0.8, 0.03)]);
        system.snow_surfaces[0].snow_depths.fill(0.01);
        system.snowballs.push(free_ball(1, 0.5, 0.4));

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        let ball = system.snowballs()[0];
        assert_eq!(ball.state, SnowballState::Resting);
        assert!((ball.y + ball.radius - 0.49).abs() < 0.002);
    }

    #[test]
    fn high_speed_snowballs_break_on_impact() {
        let mut system = ParticleSystem::new(4, 19, PhysicsConfig::default());
        let mut ball = free_ball(1, 0.5, 0.2);
        ball.velocity_y = 1.0;
        system.snowballs.push(ball);

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        assert!(
            system.snowballs().is_empty(),
            "a hard fall must break the snowball"
        );
    }

    #[test]
    fn snowballs_break_when_hitting_a_window_edge_fast() {
        let mut system = ParticleSystem::new(0, 23, PhysicsConfig::default());
        system.set_snowball_obstacles(&[BlockedRegion {
            id: 1,
            x: 0.4,
            y: 0.2,
            width: 0.3,
            height: 0.6,
        }]);
        let mut ball = free_ball(1, 0.2, 0.5);
        ball.velocity_x = 1.0;
        system.snowballs.push(ball);

        for _ in 0..120 {
            system.update(1.0 / 120.0);
        }

        assert!(
            system.snowballs().is_empty(),
            "a hard side impact must shatter the snowball"
        );
    }

    #[test]
    fn three_connected_balls_become_a_snowman() {
        let mut system = ParticleSystem::new(0, 29, PhysicsConfig::default());
        for (index, y) in [0.6_f32, 0.57, 0.54].into_iter().enumerate() {
            let mut ball = free_ball(index as u64 + 1, 0.5, y);
            ball.group = 1;
            ball.state = SnowballState::Resting;
            system.snowballs.push(ball);
        }

        system.convert_snowman_groups();

        assert!(system.snowballs().is_empty());
        assert_eq!(system.snowmen().len(), 1);
        let snowman = system.snowmen()[0];
        assert!(snowman.radius > SNOWBALL_RADIUS);
        assert!((snowman.x - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn two_connected_balls_stay_stacked_until_the_third_arrives() {
        let mut system = ParticleSystem::new(0, 31, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.6, 0.8, 0.03)]);
        system.snow_surfaces[0].snow_depths.fill(0.0);
        let mut bottom = free_ball(1, 0.5, 0.588);
        bottom.state = SnowballState::Resting;
        bottom.surface_id = 1;
        bottom.surface_offset_x = 0.4;
        system.snowballs.push(bottom);
        let mut top = free_ball(2, 0.5, 0.54);
        top.velocity_y = 0.1;
        top.placed = true;
        system.snowballs.push(top);

        for _ in 0..60 {
            system.update(1.0 / 120.0);
        }

        assert_eq!(system.snowballs().len(), 2);
        let group = system.snowballs()[0].group;
        assert_ne!(group, 0);
        assert_eq!(system.snowballs()[1].group, group);
        assert!(system.snowballs()[1].y < system.snowballs()[0].y);
    }

    #[test]
    fn dropping_three_balls_onto_each_other_builds_a_snowman() {
        let mut system = ParticleSystem::new(0, 41, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.7, 0.8, 0.03)]);
        let mut bottom = free_ball(1, 0.5, 0.7 - SNOWBALL_RADIUS);
        bottom.state = SnowballState::Resting;
        bottom.surface_id = 1;
        bottom.surface_offset_x = 0.4;
        system.snowballs.push(bottom);
        for (id, y) in [(2_u64, 0.62_f32), (3, 0.58)] {
            let mut ball = free_ball(id, 0.5, y);
            ball.velocity_y = 0.1;
            ball.placed = true;
            system.snowballs.push(ball);
        }

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        assert!(
            system.snowmen().len() == 1,
            "three stacked balls must become a snowman; balls left: {}, snowmen: {}",
            system.snowballs().len(),
            system.snowmen().len()
        );
        assert!(system.snowballs().is_empty());
    }

    #[test]
    fn incidental_collisions_do_not_glue_balls_together() {
        let mut system = ParticleSystem::new(0, 47, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.7, 0.8, 0.03)]);
        let mut bottom = free_ball(1, 0.5, 0.7 - SNOWBALL_RADIUS);
        bottom.state = SnowballState::Resting;
        bottom.surface_id = 1;
        bottom.surface_offset_x = 0.4;
        system.snowballs.push(bottom);
        let mut rolling = free_ball(2, 0.5, 0.55);
        rolling.velocity_y = 0.1;
        rolling.placed = false;
        system.snowballs.push(rolling);

        for _ in 0..120 {
            system.update(1.0 / 120.0);
        }

        assert_ne!(
            system.snowballs()[0].group,
            system.snowballs()[1].group.max(1)
        );
        assert_eq!(system.snowballs()[0].group, 0);
        assert_eq!(system.snowballs()[1].group, 0);
    }

    #[test]
    fn resting_balls_separate_sideways_without_vertical_jitter() {
        let mut system = ParticleSystem::new(0, 59, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.6, 0.8, 0.03)]);
        system.snow_surfaces[0].snow_depths.fill(0.0);
        for (id, x) in [(1_u64, 0.50_f32), (2, 0.51)] {
            let mut ball = free_ball(id, x, 0.6 - SNOWBALL_RADIUS);
            ball.state = SnowballState::Resting;
            ball.surface_id = 1;
            ball.surface_offset_x = x - 0.1;
            system.snowballs.push(ball);
        }

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        let first = system.snowballs()[0];
        let second = system.snowballs()[1];
        let dx = (first.x - second.x) * system.display_aspect;
        let dy = first.y - second.y;
        assert!(
            dx.hypot(dy) >= first.radius + second.radius - 0.0005,
            "side-by-side balls must not clip: {dx} {dy}"
        );
        assert!((first.y - (0.6 - first.radius)).abs() < 0.001);
        assert!((second.y - (0.6 - second.radius)).abs() < 0.001);
    }

    #[test]
    fn a_falling_ball_does_not_settle_inside_another_ball() {
        let mut system = ParticleSystem::new(0, 61, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.7, 0.8, 0.03)]);
        system.snow_surfaces[0].snow_depths.fill(0.0);
        let mut bottom = free_ball(1, 0.5, 0.7 - SNOWBALL_RADIUS);
        bottom.state = SnowballState::Resting;
        bottom.surface_id = 1;
        bottom.surface_offset_x = 0.4;
        system.snowballs.push(bottom);
        let mut falling = free_ball(2, 0.5, 0.55);
        falling.velocity_y = 0.22;
        system.snowballs.push(falling);

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        assert_eq!(system.snowballs().len(), 2);
        let first = system.snowballs()[0];
        let second = system.snowballs()[1];
        let dx = (first.x - second.x) * system.display_aspect;
        let dy = first.y - second.y;
        assert!(
            dx.hypot(dy) >= first.radius + second.radius - 0.0005,
            "balls must not overlap after landing: {dx} {dy}"
        );
    }

    #[test]
    fn held_ball_cannot_pass_through_another_ball() {
        let mut system = ParticleSystem::new(0, 71, PhysicsConfig::default());
        system.snowballs.push(free_ball(1, 0.50, 0.50));
        system.snowballs.push(free_ball(2, 0.52, 0.50));
        let id = system.grab_snowball(0.50, 0.50).expect("grab held ball");
        assert!(system.drag_snowball(id, 0.52, 0.50, 1.0 / 120.0));
        let first = system.snowballs()[0];
        let second = system.snowballs()[1];
        let dx = (first.x - second.x) * system.display_aspect;
        let dy = first.y - second.y;
        assert!(
            dx.hypot(dy) >= first.radius + second.radius - 0.0005,
            "the held ball clipped through its neighbour: {dx} {dy}"
        );
    }

    #[test]
    fn held_ball_is_clamped_to_the_screen_edges() {
        let mut system = ParticleSystem::new(0, 73, PhysicsConfig::default());
        system.snowballs.push(free_ball(1, 0.5, 0.5));
        let id = system.grab_snowball(0.5, 0.5).expect("grab held ball");
        assert!(system.drag_snowball(id, -1.0, 2.0, 1.0 / 120.0));
        let ball = system.snowballs()[0];
        assert!((ball.x - ball.radius / system.display_aspect).abs() < 0.0005);
        assert!((ball.y - (1.0 - ball.radius)).abs() < 0.0005);
    }

    #[test]
    fn held_ball_is_blocked_by_a_snowman() {
        let mut system = ParticleSystem::new(0, 79, PhysicsConfig::default());
        system.snowballs.push(free_ball(1, 0.5, 0.5));
        system.snowmen.push(Snowman {
            id: 1,
            x: 0.5,
            y: 0.7,
            radius: 0.025,
            surface_id: NO_SURFACE_ID,
            surface_offset_x: 0.0,
            held: false,
            velocity_x: 0.0,
            velocity_y: 0.0,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
            drag_offset_x: 0.0,
            drag_offset_y: 0.0,
        });
        let id = system.grab_snowball(0.5, 0.5).expect("grab held ball");
        assert!(system.drag_snowball(id, 0.5, 0.68, 1.0 / 120.0));
        let ball = system.snowballs()[0];
        let (cx, top, bottom, capsule_radius) = snowman_collision_capsule(&system.snowmen()[0]);
        let ny = ball.y.clamp(top, bottom);
        let dx = (ball.x - cx) * system.display_aspect;
        let dy = ball.y - ny;
        assert!(
            dx.hypot(dy) >= ball.radius + capsule_radius - 0.0005,
            "held ball entered a snowman body"
        );
    }

    #[test]
    fn free_ball_is_pushed_out_of_a_snowman() {
        let mut system = ParticleSystem::new(0, 81, PhysicsConfig::default());
        system.snowmen.push(Snowman {
            id: 1,
            x: 0.5,
            y: 0.7,
            radius: 0.025,
            surface_id: NO_SURFACE_ID,
            surface_offset_x: 0.0,
            held: false,
            velocity_x: 0.0,
            velocity_y: 0.0,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
            drag_offset_x: 0.0,
            drag_offset_y: 0.0,
        });
        system.snowballs.push(free_ball(2, 0.5, 0.66));
        system.update(1.0 / 120.0);
        let ball = system.snowballs()[0];
        let (cx, top, bottom, capsule_radius) = snowman_collision_capsule(&system.snowmen()[0]);
        let ny = ball.y.clamp(top, bottom);
        let dx = (ball.x - cx) * system.display_aspect;
        let dy = ball.y - ny;
        assert!(
            dx.hypot(dy) >= ball.radius + capsule_radius - 0.002,
            "free ball settled inside a snowman body"
        );
    }

    #[test]
    fn snowball_limit_is_exposed() {
        let mut system = ParticleSystem::new(0, 83, PhysicsConfig::default());
        for id in 0..MAX_SNOWBALLS as u64 {
            system.snowballs.push(free_ball(id + 1, 0.5, 0.5));
        }
        assert!(system.snowball_limit_reached());
    }

    #[test]
    fn snow_moving_under_a_resting_ball_is_followed_smoothly() {
        let mut system = ParticleSystem::new(0, 67, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.6, 0.8, 0.05)]);
        system.snow_surfaces[0].snow_depths.fill(0.0);
        let mut ball = free_ball(1, 0.5, 0.6 - SNOWBALL_RADIUS);
        ball.state = SnowballState::Resting;
        ball.surface_id = 1;
        ball.surface_offset_x = 0.4;
        system.snowballs.push(ball);

        system.snow_surfaces[0].snow_depths.fill(0.02);
        system.update(1.0 / 120.0);
        let followed = system.snowballs()[0].y;
        let target = 0.6 - 0.02 - SNOWBALL_RADIUS;
        assert!(followed < 0.6 - SNOWBALL_RADIUS);
        assert!(
            followed > target + 0.0001,
            "the ball should ease toward the new snow height, got {followed}, target {target}"
        );

        for _ in 0..120 {
            system.update(1.0 / 120.0);
        }
        assert!((system.snowballs()[0].y - target).abs() < 0.001);
    }

    #[test]
    fn snowmen_can_be_grabbed_dragged_and_dropped() {
        let mut system = ParticleSystem::new(0, 53, PhysicsConfig::default());
        system.set_surfaces(&[flat_surface(1, 0.1, 0.7, 0.8, 0.03)]);
        system.snowmen.push(Snowman {
            id: 1,
            x: 0.5,
            y: 0.7,
            radius: 0.02,
            surface_id: 1,
            surface_offset_x: 0.4,
            held: false,
            velocity_x: 0.0,
            velocity_y: 0.0,
            drag_velocity_x: 0.0,
            drag_velocity_y: 0.0,
            drag_offset_x: 0.0,
            drag_offset_y: 0.0,
        });

        let id = system
            .grab_snowman(0.5, 0.66)
            .expect("the snowman body should be grabbable");
        assert!(system.snowmen()[0].held);
        assert!(system.drag_snowman(id, 0.45, 0.69, 0.05));
        assert!((system.snowmen()[0].x - 0.45).abs() < 0.02);
        assert!(system.release_snowman(id));
        assert!(!system.snowmen()[0].held);

        for _ in 0..240 {
            system.update(1.0 / 120.0);
        }

        let snowman = system.snowmen()[0];
        assert!(
            (snowman.y - 0.7).abs() < 0.02,
            "snowman should land on the surface, y={} x={} surface_id={}",
            snowman.y,
            snowman.x,
            snowman.surface_id
        );
        assert_eq!(snowman.surface_id, 1);
    }

    #[test]
    fn dragging_a_snowman_outside_the_display_keeps_it_on_screen() {
        let mut system = ParticleSystem::new(0, 89, PhysicsConfig::default());
        system.snowmen.push(Snowman {
            id: 1,
            x: 0.5,
            y: 0.7,
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
        });
        let id = system.grab_snowman(0.5, 0.7).expect("grab snowman");
        assert!(system.drag_snowman(id, -10.0, -10.0, 0.05));
        let snowman = system.snowmen()[0];
        let radius_x = snowman.radius / system.display_aspect;
        assert!(snowman.x >= radius_x - 0.0005);
        assert!(snowman.y >= snowman.radius * 4.7 - 0.0005);
        assert!(system.drag_snowman(id, 10.0, 10.0, 0.05));
        let snowman = system.snowmen()[0];
        assert!(snowman.x <= 1.0 - radius_x + 0.0005);
        assert!(snowman.y <= 1.0 + 0.0005);
    }

    #[test]
    fn clearing_snow_also_removes_balls_and_snowmen() {
        let mut system = ParticleSystem::new(0, 37, PhysicsConfig::default());
        system.snowballs.push(free_ball(1, 0.5, 0.5));
        system.snowmen.push(Snowman {
            id: 1,
            x: 0.5,
            y: 0.6,
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
        });

        system.clear_snow();

        assert!(system.snowballs().is_empty());
        assert!(system.snowmen().is_empty());
    }
}
