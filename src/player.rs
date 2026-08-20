//! First-person physics character: dynamic capsule with locked rotation,
//! velocity-driven movement, mouse look, jump, sprint and cursor management.

use avian3d::prelude::*;
use bevy::camera::{Exposure, Hdr};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::pbr::DistanceFog;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::citygen::CityMeta;
use crate::AppState;

pub const EYE_HEIGHT_OFFSET: f32 = 0.65;
const CAPSULE_RADIUS: f32 = 0.35;
const CAPSULE_LENGTH: f32 = 1.0; // cylinder part; total height = 1.7
const WALK_SPEED: f32 = 5.8;
const SPRINT_SPEED: f32 = 9.6;
const JUMP_SPEED: f32 = 7.4;
const MOUSE_SENS: f32 = 0.0021;
const BASE_FOV_DEG: f32 = 62.0;
const SPRINT_FOV_DEG: f32 = 70.0;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CursorGrabbed(false))
            .add_systems(
                OnEnter(AppState::Playing),
                spawn_player.after(crate::citygen::build_city),
            )
            .add_systems(
                Update,
                (
                    cursor_input,
                    sync_cursor,
                    mouse_look,
                    movement,
                    fov_kick,
                    respawn_if_fallen,
                )
                    .chain()
                    .in_set(crate::GameSet::Player)
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

/// Whether the mouse is captured for FPS look. Other modules (e.g. the
/// inspector overlay) flip this to release the cursor.
#[derive(Resource)]
pub struct CursorGrabbed(pub bool);

#[derive(Component)]
pub struct Player {
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
}

#[derive(Component)]
pub struct PlayerCamera;

fn spawn_player(mut commands: Commands, meta: Res<CityMeta>) {
    let fog_end = (meta.half_extent * 2.2).clamp(160.0, 520.0);
    commands
        .spawn((
            Transform::from_translation(meta.spawn_pos),
            Visibility::default(),
            RigidBody::Dynamic,
            Collider::capsule(CAPSULE_RADIUS, CAPSULE_LENGTH),
            LockedAxes::ROTATION_LOCKED,
            Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
            Restitution::new(0.0).with_combine_rule(CoefficientCombine::Min),
            GravityScale(1.7),
            Mass(75.0),
            TransformInterpolation,
            Player {
                yaw: 0.0,
                pitch: -0.05,
                grounded: false,
            },
        ))
        .with_children(|parent| {
            parent.spawn((
                Camera3d::default(),
                Hdr,
                // Required for the procedural sky to render on this camera.
                bevy::pbr::AtmosphereSettings::default(),
                // Sky-driven ambient light and reflections.
                bevy::light::AtmosphereEnvironmentMapLight::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: BASE_FOV_DEG.to_radians(),
                    ..default()
                }),
                Transform::from_xyz(0.0, EYE_HEIGHT_OFFSET, 0.0),
                Bloom::NATURAL,
                Exposure { ev100: 9.7 },
                DistanceFog {
                    color: Color::srgb(0.62, 0.74, 0.88),
                    falloff: FogFalloff::Linear {
                        start: fog_end * 0.35,
                        end: fog_end,
                    },
                    ..default()
                },
                PlayerCamera,
            ));
        });
}

fn cursor_input(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut grab: ResMut<CursorGrabbed>,
    inspector: Res<crate::interact::Inspector>,
) {
    // While the inspector overlay is open, it owns Esc/click handling.
    if inspector.0.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) && grab.0 {
        grab.0 = false;
    } else if mouse.just_pressed(MouseButton::Left) && !grab.0 {
        grab.0 = true;
    }
}

fn sync_cursor(
    grab: Res<CursorGrabbed>,
    mut options: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if !grab.is_changed() {
        return;
    }
    if grab.0 {
        options.grab_mode = CursorGrabMode::Locked;
        options.visible = false;
    } else {
        options.grab_mode = CursorGrabMode::None;
        options.visible = true;
    }
}

fn mouse_look(
    grab: Res<CursorGrabbed>,
    motion: Res<AccumulatedMouseMotion>,
    mut player: Query<&mut Player>,
    mut camera: Query<&mut Transform, With<PlayerCamera>>,
) {
    let Ok(mut player) = player.single_mut() else {
        return;
    };
    if grab.0 {
        let delta = motion.delta;
        player.yaw -= delta.x * MOUSE_SENS;
        player.pitch = (player.pitch - delta.y * MOUSE_SENS).clamp(-1.54, 1.54);
    }
    if let Ok(mut cam) = camera.single_mut() {
        cam.rotation = Quat::from_euler(EulerRot::YXZ, player.yaw, player.pitch, 0.0);
    }
}

fn movement(
    keys: Res<ButtonInput<KeyCode>>,
    grab: Res<CursorGrabbed>,
    spatial: SpatialQuery,
    mut query: Query<(Entity, &Transform, &mut LinearVelocity, &mut Player)>,
    time: Res<Time>,
) {
    let Ok((entity, transform, mut velocity, mut player)) = query.single_mut() else {
        return;
    };

    // Grounded: short ray straight down from the capsule center.
    let filter = SpatialQueryFilter::default().with_excluded_entities([entity]);
    let half = CAPSULE_LENGTH * 0.5 + CAPSULE_RADIUS;
    player.grounded = spatial
        .cast_ray(
            transform.translation,
            Dir3::NEG_Y,
            half + 0.18,
            true,
            &filter,
        )
        .is_some();

    let mut wish = Vec3::ZERO;
    if grab.0 {
        let forward = Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos());
        let right = Vec3::new(-player.yaw.cos(), 0.0, player.yaw.sin()) * -1.0;
        if keys.pressed(KeyCode::KeyW) {
            wish += forward;
        }
        if keys.pressed(KeyCode::KeyS) {
            wish -= forward;
        }
        if keys.pressed(KeyCode::KeyD) {
            wish += right;
        }
        if keys.pressed(KeyCode::KeyA) {
            wish -= right;
        }
    }
    let sprinting = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let speed = if sprinting { SPRINT_SPEED } else { WALK_SPEED };
    let target = wish.normalize_or_zero() * speed;

    let rate = if player.grounded { 12.0 } else { 3.0 };
    let alpha = 1.0 - (-rate * time.delta_secs()).exp();
    velocity.x += (target.x - velocity.x) * alpha;
    velocity.z += (target.z - velocity.z) * alpha;

    if grab.0 && player.grounded && keys.just_pressed(KeyCode::Space) {
        velocity.y = JUMP_SPEED;
    }
}

fn fov_kick(
    keys: Res<ButtonInput<KeyCode>>,
    velocity: Query<&LinearVelocity, With<Player>>,
    mut projection: Query<&mut Projection, With<PlayerCamera>>,
    time: Res<Time>,
) {
    let Ok(velocity) = velocity.single() else {
        return;
    };
    let Ok(mut projection) = projection.single_mut() else {
        return;
    };
    let sprinting = (keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight))
        && velocity.xz().length() > 6.5;
    let target = if sprinting {
        SPRINT_FOV_DEG
    } else {
        BASE_FOV_DEG
    }
    .to_radians();
    if let Projection::Perspective(persp) = &mut *projection {
        let alpha = 1.0 - (-10.0 * time.delta_secs()).exp();
        persp.fov += (target - persp.fov) * alpha;
    }
}

/// Safety net: teleport back to spawn if the player somehow falls off the map.
fn respawn_if_fallen(
    meta: Res<CityMeta>,
    mut query: Query<(&mut Transform, &mut LinearVelocity), With<Player>>,
) {
    for (mut transform, mut velocity) in &mut query {
        if transform.translation.y < -30.0 {
            transform.translation = meta.spawn_pos;
            velocity.0 = Vec3::ZERO;
        }
    }
}
