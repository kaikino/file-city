//! Dynamic sky and time of day: a physically-based atmosphere, a sun that
//! sweeps through a full day-night cycle, street lamps that switch on at
//! dusk, and shared materials (neon, windows) that brighten at night.

use bevy::camera::Exposure;
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{Atmosphere, CascadeShadowConfigBuilder};
use bevy::pbr::DistanceFog;
use bevy::prelude::*;

use crate::citygen::{CityMeta, Gates, Palette, NEON_COLORS};
use crate::AppState;

/// Full day length in seconds.
const DAY_SECONDS: f32 = 480.0;
/// Start just after sunset: twilight glow with the neon coming alive.
const START_T: f32 = 0.77;
const MAX_LAMPS: usize = 56;

pub struct AtmoPlugin;

impl Plugin for AtmoPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TimeOfDay { t: START_T })
            .insert_resource(NightFactor(1.0))
            .add_systems(Startup, apply_tod_flag)
            .add_systems(OnEnter(AppState::Playing), setup_sky)
            .add_systems(
                Update,
                (spawn_street_lamps, day_night_cycle, fly_drones)
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

/// Normalized time of day: 0 = midnight, 0.25 = sunrise, 0.5 = noon.
#[derive(Resource)]
pub struct TimeOfDay {
    pub t: f32,
}

/// 0 in full daylight, 1 at night; drives neon/window/lamp intensity.
#[derive(Resource)]
pub struct NightFactor(pub f32);

fn apply_tod_flag(cfg: Res<crate::scan::ScanConfig>, mut tod: ResMut<TimeOfDay>) {
    if let Some(t) = cfg.tod {
        tod.t = t.rem_euclid(1.0);
    }
}

#[derive(Component)]
struct Sun;

#[derive(Component)]
struct StreetLamp;

/// Glowing traffic streak circling above the rooftops.
#[derive(Component)]
struct Drone {
    center: Vec2,
    radius: f32,
    height: f32,
    speed: f32,
    phase: f32,
}

fn setup_sky(
    mut commands: Commands,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    ambient.color = Color::srgb(0.6, 0.62, 0.9);
    ambient.brightness = 80.0;
    commands.spawn((
        Atmosphere::earth(mediums.add(ScatteringMedium::default())),
        Transform::default(),
    ));
    commands.spawn((
        Sun,
        DirectionalLight {
            color: Color::srgb(1.0, 0.6, 0.3),
            illuminance: 400.0,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            maximum_distance: 170.0,
            first_cascade_far_bound: 14.0,
            ..default()
        }
        .build(),
        Transform::default(),
    ));
}

/// Places lamp posts at street gates and launches drones, once the city
/// exists.
fn spawn_street_lamps(
    mut commands: Commands,
    gates: Option<Res<Gates>>,
    meta: Option<Res<CityMeta>>,
    meshes: Option<Res<crate::citygen::CityMeshes>>,
    palette: Option<Res<Palette>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let (Some(gates), Some(meta), Some(meshes), Some(palette)) = (gates, meta, meshes, palette)
    else {
        return;
    };
    *done = true;
    let step = (gates.0.len() / MAX_LAMPS).max(1);
    let mut count = 0;
    for (i, (gate, out)) in gates.0.iter().step_by(step).enumerate() {
        if i >= MAX_LAMPS {
            break;
        }
        // Alternate warm sodium and cool neon-tinted lights.
        let color = if i % 3 == 0 {
            Color::srgb(1.0, 0.44, 0.75)
        } else {
            Color::srgb(1.0, 0.75, 0.45)
        };
        // Stand the pole beside the gate, just off the walking line.
        let side = Vec2::new(out.y, -out.x) * 1.5;
        commands
            .spawn((
                Mesh3d(meshes.cube.clone()),
                MeshMaterial3d(palette.dark_metal.clone()),
                Transform::from_xyz(gate.x + side.x, gate.y + 1.9, gate.z + side.y)
                    .with_scale(Vec3::new(0.09, 3.8, 0.09)),
            ))
            .with_children(|parent| {
                parent.spawn((
                    StreetLamp,
                    PointLight {
                        color,
                        intensity: 0.0,
                        range: 20.0,
                        ..default()
                    },
                    // Child of a scaled pole; local offset puts it at the top.
                    Transform::from_xyz(0.0, 0.52, 0.0),
                ));
            });
        count += 1;
    }
    info!("street lamps: {count}");

    // Drones circling over the city at rooftop-plus heights.
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32
    };
    for i in 0..14 {
        let r = meta.half_extent * (0.25 + 0.6 * next());
        let drone = Drone {
            center: Vec2::new(
                (next() - 0.5) * meta.half_extent,
                (next() - 0.5) * meta.half_extent,
            ),
            radius: r,
            height: 26.0 + 22.0 * next(),
            speed: (2.5 + 4.5 * next()) / r,
            phase: next() * std::f32::consts::TAU,
        };
        commands.spawn((
            Mesh3d(meshes.cube.clone()),
            MeshMaterial3d(palette.neon[i % 6].clone()),
            Transform::from_xyz(drone.center.x, drone.height, drone.center.y)
                .with_scale(Vec3::new(0.5, 0.14, 1.3)),
            bevy::light::NotShadowCaster,
            drone,
        ));
    }
}

/// Moves drones along their circular lanes, nose pointed along the path.
fn fly_drones(time: Res<Time>, mut drones: Query<(&Drone, &mut Transform)>) {
    let t = time.elapsed_secs();
    for (drone, mut transform) in &mut drones {
        let a = drone.phase + t * drone.speed;
        let (sin, cos) = a.sin_cos();
        let pos = drone.center + Vec2::new(cos, sin) * drone.radius;
        let tangent = Vec2::new(-sin, cos);
        transform.translation = Vec3::new(pos.x, drone.height + (t * 0.7 + drone.phase).sin(), pos.y);
        transform.rotation = Quat::from_rotation_y(tangent.x.atan2(tangent.y))
            * Quat::from_rotation_z(0.28);
    }
}

fn day_night_cycle(
    time: Res<Time>,
    mut tod: ResMut<TimeOfDay>,
    mut night: ResMut<NightFactor>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), With<Sun>>,
    mut cameras: Query<(&mut Exposure, Option<&mut DistanceFog>), With<Camera3d>>,
    mut lamps: Query<&mut PointLight, With<StreetLamp>>,
    meta: Option<Res<CityMeta>>,
    palette: Option<Res<Palette>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    tod.t = (tod.t + time.delta_secs() / DAY_SECONDS).fract();
    let two_pi = std::f32::consts::TAU;
    let sin_e = (two_pi * (tod.t - 0.25)).sin();
    let elev = sin_e * 62f32.to_radians();
    // Azimuth sweeps east to west through the day.
    let day_frac = (tod.t - 0.25) / 0.5;
    let azi = (-100.0 + 200.0 * day_frac).to_radians();

    let nf = (1.0 - (sin_e + 0.05) / 0.2).clamp(0.0, 1.0);
    night.0 = nf;

    if let Ok((mut transform, mut light)) = sun.single_mut() {
        transform.rotation = Quat::from_euler(EulerRot::YXZ, azi, -elev, 0.0);
        light.illuminance = if sin_e > 0.0 {
            500.0 + 10_500.0 * sin_e.powf(0.8)
        } else {
            500.0 * (1.0 + sin_e / 0.12).clamp(0.0, 1.0)
        };
        let horizon = (1.0 - (sin_e.abs() * 3.0).clamp(0.0, 1.0)).powi(2);
        let c = Vec3::new(1.0, 0.95, 0.88).lerp(Vec3::new(1.0, 0.45, 0.22), horizon);
        light.color = Color::srgb(c.x, c.y, c.z);
    }

    let amb = Vec3::new(0.75, 0.85, 1.0).lerp(Vec3::new(0.50, 0.42, 0.85), nf);
    ambient.color = Color::srgb(amb.x, amb.y, amb.z);
    ambient.brightness = 260.0 - 210.0 * nf;

    let fog_end_day = meta
        .map(|m| (m.half_extent * 2.4).clamp(150.0, 420.0))
        .unwrap_or(260.0);
    for (mut exposure, fog) in &mut cameras {
        exposure.ev100 = 9.7 - 1.8 * nf;
        if let Some(mut fog) = fog {
            let f = Vec3::new(0.62, 0.74, 0.88).lerp(Vec3::new(0.05, 0.045, 0.11), nf);
            fog.color = Color::srgb(f.x, f.y, f.z);
            let end = fog_end_day * (1.0 - 0.42 * nf);
            fog.falloff = FogFalloff::Linear {
                start: end * (0.35 - 0.16 * nf),
                end,
            };
        }
    }

    for mut lamp in &mut lamps {
        lamp.intensity = 90_000.0 * nf;
    }

    // Neon and lit windows breathe with the dark.
    if let Some(palette) = palette {
        for (i, handle) in palette.neon.iter().enumerate() {
            if let Some(mut mat) = materials.get_mut(handle) {
                mat.emissive = LinearRgba::from(NEON_COLORS[i]) * (2.2 + 4.3 * nf);
            }
        }
        if let Some(mut mat) = materials.get_mut(&palette.window_lit) {
            mat.emissive = LinearRgba::rgb(1.4, 1.05, 0.55) * (0.25 + 1.35 * nf);
        }
    }
}
