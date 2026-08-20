//! Constructs one building per file: hash-seeded tiers, rooftop clutter,
//! window strips, awnings, neon signage and per-kind screens on multiple
//! faces. Local +Z is the street-facing side; the root entity is unscaled and
//! carries the collider via a child, so children use real-world sizes.

use avian3d::prelude::*;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::citygen::{
    seed_for, Beacon, Bobber, FileRef, ImageScreen, NeonNameSign, Prop, SpawnCtx, TextScreen,
};
use crate::scan::{FileEntry, FileKind};

/// Meshes belonging to the building body: these swap to the highlight
/// material when hovered.
#[derive(Component)]
pub struct BuildingBody;

pub fn spawn_building(
    ctx: &mut SpawnCtx,
    f: &FileEntry,
    base: Vec3,
    w: f32,
    d: f32,
    yaw: f32,
    rng: &mut SmallRng,
) {
    let kind = f.kind;
    let size_term = (f.size as f32 + 1.0).log2();
    let kind_mult = match kind {
        FileKind::Data => 1.35,
        FileKind::Archive => 0.45,
        FileKind::Video => 0.85,
        _ => 1.0,
    };
    let h = ((3.2 + size_term * 0.8) * (0.75 + rng.random_range(0.0..0.55)) * kind_mult)
        .clamp(3.2, 26.0);
    let shade = rng.random_range(0..4usize);
    let body_mat = ctx.palette.body[&kind][shade].clone();

    let file_ref = FileRef {
        name: f.name.clone(),
        path: f.path.clone(),
        size: f.size,
        kind,
    };

    // Tier heights: 1-3 stacked boxes with setbacks.
    let tiers: Vec<(f32, f32, f32)> = if h > 10.0 && rng.random_bool(0.45) {
        if h > 16.0 && rng.random_bool(0.3) {
            let h1 = h * 0.55;
            let h2 = h * 0.28;
            vec![(w, h1, d), (w * 0.78, h2, d * 0.8), (w * 0.55, h - h1 - h2, d * 0.6)]
        } else {
            let h1 = h * rng.random_range(0.6..0.75);
            vec![(w, h1, d), (w * 0.76, h - h1, d * 0.78)]
        }
    } else {
        vec![(w, h, d)]
    };
    let h1 = tiers[0].1;

    let root = ctx
        .commands
        .spawn((
            Transform::from_translation(base).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
            RigidBody::Static,
            file_ref,
        ))
        .id();

    let mut children: Vec<Entity> = Vec::new();
    let cmd = &mut *ctx.commands;

    // Collider covering the full stack (attaches to the root body).
    children.push(
        cmd.spawn((
            Transform::from_xyz(0.0, h * 0.5, 0.0),
            Collider::cuboid(w, h, d),
        ))
        .id(),
    );

    // Tier boxes.
    let mut y = 0.0;
    for (i, (tw, th, td)) in tiers.iter().copied().enumerate() {
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.cube.clone()),
                MeshMaterial3d(body_mat.clone()),
                Transform::from_xyz(0.0, y + th * 0.5, 0.0).with_scale(Vec3::new(tw, th, td)),
                BuildingBody,
            ))
            .id(),
        );
        y += th;
        // Parapet lip on each tier edge.
        if i + 1 == tiers.len() || rng.random_bool(0.5) {
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.roof.clone()),
                    Transform::from_xyz(0.0, y + 0.09, 0.0)
                        .with_scale(Vec3::new(tw + 0.14, 0.2, td + 0.14)),
                ))
                .id(),
            );
        }
    }
    let (top_w, _, top_d) = *tiers.last().unwrap();

    // --- Rooftop clutter ---------------------------------------------------
    if rng.random_bool(0.5) && top_w > 2.2 {
        // Water tank.
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.cylinder.clone()),
                MeshMaterial3d(ctx.palette.dark_metal.clone()),
                Transform::from_xyz(
                    rng.random_range(-0.25..0.25) * top_w,
                    h + 0.55,
                    rng.random_range(-0.25..0.25) * top_d,
                )
                .with_scale(Vec3::new(1.1, 1.1, 1.1)),
            ))
            .id(),
        );
    }
    for _ in 0..rng.random_range(0..3u32) {
        // AC boxes.
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.cube.clone()),
                MeshMaterial3d(ctx.palette.dark_metal.clone()),
                Transform::from_xyz(
                    rng.random_range(-0.32..0.32) * top_w,
                    h + 0.3,
                    rng.random_range(-0.32..0.32) * top_d,
                )
                .with_scale(Vec3::new(0.7, 0.6, 0.6)),
            ))
            .id(),
        );
    }
    let wants_antenna = h > 14.0 || kind == FileKind::Data || rng.random_bool(0.25);
    if wants_antenna {
        let ah = rng.random_range(1.5..3.5);
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.cube.clone()),
                MeshMaterial3d(ctx.palette.dark_metal.clone()),
                Transform::from_xyz(top_w * 0.2, h + ah * 0.5, 0.0)
                    .with_scale(Vec3::new(0.09, ah, 0.09)),
            ))
            .id(),
        );
        if h > 13.0 {
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.sphere.clone()),
                    MeshMaterial3d(ctx.palette.beacon.clone()),
                    Transform::from_xyz(top_w * 0.2, h + ah + 0.1, 0.0)
                        .with_scale(Vec3::splat(0.24)),
                    Beacon,
                    NotShadowCaster,
                ))
                .id(),
            );
        }
    }

    // --- Facade dressing ----------------------------------------------------
    // Window strips on street and back faces.
    let mut fy = 3.1;
    while fy < h1 - 1.4 && w > 2.6 {
        for (z, flip) in [(d * 0.5 + 0.015, false), (-d * 0.5 - 0.015, true)] {
            let lit = rng.random_bool(0.58);
            let mat = if lit {
                ctx.palette.window_lit.clone()
            } else {
                ctx.palette.window_dark.clone()
            };
            let mut t = Transform::from_xyz(0.0, fy, z).with_scale(Vec3::new(w * 0.84, 0.62, 1.0));
            if flip {
                t.rotation = Quat::from_rotation_y(std::f32::consts::PI);
            }
            children.push(
                cmd.spawn((Mesh3d(ctx.meshes.quad.clone()), MeshMaterial3d(mat), t, NotShadowCaster))
                    .id(),
            );
        }
        fy += 2.7;
    }

    // Ground-floor storefront glass.
    children.push(
        cmd.spawn((
            Mesh3d(ctx.meshes.quad.clone()),
            MeshMaterial3d(ctx.palette.window_dark.clone()),
            Transform::from_xyz(0.0, 1.25, d * 0.5 + 0.012)
                .with_scale(Vec3::new(w * 0.82, 1.5, 1.0)),
            NotShadowCaster,
        ))
        .id(),
    );

    // Awning over the entrance.
    if rng.random_bool(0.45) {
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.cube.clone()),
                MeshMaterial3d(ctx.palette.awning[rng.random_range(0..2usize)].clone()),
                Transform::from_xyz(0.0, 2.3, d * 0.5 + 0.42)
                    .with_rotation(Quat::from_rotation_x(0.22))
                    .with_scale(Vec3::new(w * 0.9, 0.06, 0.9)),
            ))
            .id(),
        );
    }

    // Vertical neon name sign protruding from the facade corner.
    if rng.random_bool(0.6) {
        let sh = (h * 0.45).clamp(2.0, 4.6);
        let side = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
        let seed = seed_for(&f.path);
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.quad.clone()),
                MeshMaterial3d(ctx.palette.neon[(seed % 6) as usize].clone()),
                Transform::from_xyz(side * (w * 0.5 + 0.42), (h1 * 0.55).max(sh * 0.55 + 1.8), d * 0.28)
                    .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::new(0.72, sh, 1.0)),
                NeonNameSign {
                    name: f.name.clone(),
                    hue_seed: seed,
                },
                NotShadowCaster,
            ))
            .id(),
        );
    }

    // Random static neon bars.
    for _ in 0..rng.random_range(0..3u32) {
        let flicker = rng.random_bool(0.18);
        let mat = if flicker {
            ctx.palette.neon_flicker.clone()
        } else {
            ctx.palette.neon[rng.random_range(0..6usize)].clone()
        };
        children.push(
            cmd.spawn((
                Mesh3d(ctx.meshes.quad.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(
                    rng.random_range(-0.3..0.3) * w,
                    rng.random_range(2.4..(h1 - 0.5).max(2.6)),
                    d * 0.5 + 0.02,
                )
                .with_scale(Vec3::new(w * rng.random_range(0.25..0.6), 0.16, 1.0)),
                NotShadowCaster,
            ))
            .id(),
        );
    }

    // --- Kind-specific features ---------------------------------------------
    match kind {
        FileKind::Text | FileKind::Code => {
            for (sw, sh, sy, z, flip) in [
                (w * 0.7, h1 * 0.48, h1 * 0.45, d * 0.5 + 0.03, false),
                (w * 0.52, h1 * 0.36, h1 * 0.52, -d * 0.5 - 0.03, true),
            ] {
                let mut t = Transform::from_xyz(0.0, sy, z).with_scale(Vec3::new(sw, sh, 1.0));
                if flip {
                    t.rotation = Quat::from_rotation_y(std::f32::consts::PI);
                }
                children.push(
                    cmd.spawn((
                        Mesh3d(ctx.meshes.quad.clone()),
                        MeshMaterial3d(ctx.palette.screen_off.clone()),
                        t,
                        TextScreen {
                            path: f.path.clone(),
                            kind,
                        },
                        NotShadowCaster,
                    ))
                    .id(),
                );
            }
        }
        FileKind::Image => {
            let (sw, sh) = (w * 0.78, h1 * 0.5);
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.quad.clone()),
                    MeshMaterial3d(ctx.palette.screen_off.clone()),
                    Transform::from_xyz(0.0, h1 * 0.5, d * 0.5 + 0.03)
                        .with_scale(Vec3::new(sw, sh, 1.0)),
                    ImageScreen {
                        path: f.path.clone(),
                        base_size: Vec2::new(sw, sh),
                    },
                    NotShadowCaster,
                ))
                .id(),
            );
            if d > 3.4 && rng.random_bool(0.6) {
                let side = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
                let (sw2, sh2) = (d * 0.62, h1 * 0.4);
                children.push(
                    cmd.spawn((
                        Mesh3d(ctx.meshes.quad.clone()),
                        MeshMaterial3d(ctx.palette.screen_off.clone()),
                        Transform::from_xyz(side * (w * 0.5 + 0.03), h1 * 0.55, 0.0)
                            .with_rotation(Quat::from_rotation_y(side * std::f32::consts::FRAC_PI_2))
                            .with_scale(Vec3::new(sw2, sh2, 1.0)),
                        ImageScreen {
                            path: f.path.clone(),
                            base_size: Vec2::new(sw2, sh2),
                        },
                        NotShadowCaster,
                    ))
                    .id(),
                );
            }
        }
        FileKind::Video => {
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.quad.clone()),
                    MeshMaterial3d(ctx.palette.screen_off.clone()),
                    Transform::from_xyz(0.0, h * 0.62, d * 0.5 + 0.03)
                        .with_scale(Vec3::new(w * 0.86, h * 0.3, 1.0)),
                    NotShadowCaster,
                ))
                .id(),
            );
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.marquee.clone()),
                    Transform::from_xyz(0.0, h * 0.8, d * 0.5 + 0.12)
                        .with_scale(Vec3::new(w * 1.02, 0.22, 0.24)),
                    NotShadowCaster,
                ))
                .id(),
            );
        }
        FileKind::Audio => {
            // Orb on a rooftop pole.
            let orb_d = (w * 0.32).clamp(0.7, 1.5);
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.dark_metal.clone()),
                    Transform::from_xyz(0.0, h + 0.7, 0.0).with_scale(Vec3::new(0.09, 1.4, 0.09)),
                ))
                .id(),
            );
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.sphere.clone()),
                    MeshMaterial3d(ctx.palette.orb.clone()),
                    Transform::from_xyz(0.0, h + 1.65, 0.0).with_scale(Vec3::splat(orb_d)),
                    Bobber {
                        base_y: h + 1.65,
                        phase: base.x * 0.7,
                        amp: 0.2,
                    },
                    NotShadowCaster,
                ))
                .id(),
            );
        }
        FileKind::Archive => {
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.gold_trim.clone()),
                    Transform::from_xyz(0.0, h * 0.5, 0.0)
                        .with_scale(Vec3::new(w + 0.08, 0.28, d + 0.08)),
                ))
                .id(),
            );
        }
        FileKind::Executable => {
            // Robot statue on the roof.
            let s = (w * 0.3).clamp(0.6, 1.1);
            let robot_y = h + s * 0.75;
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(body_mat.clone()),
                    Transform::from_xyz(0.0, robot_y, 0.0)
                        .with_scale(Vec3::new(s, s * 1.1, s * 0.6)),
                ))
                .id(),
            );
            children.push(
                cmd.spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(body_mat.clone()),
                    Transform::from_xyz(0.0, robot_y + s * 0.9, 0.0)
                        .with_scale(Vec3::splat(s * 0.55)),
                ))
                .id(),
            );
            for ex in [-0.16, 0.16] {
                children.push(
                    cmd.spawn((
                        Mesh3d(ctx.meshes.cube.clone()),
                        MeshMaterial3d(ctx.palette.eye.clone()),
                        Transform::from_xyz(ex * s, robot_y + s * 0.95, s * 0.3)
                            .with_scale(Vec3::splat(s * 0.12)),
                        NotShadowCaster,
                    ))
                    .id(),
                );
            }
        }
        FileKind::Data | FileKind::Other => {}
    }

    for child in children {
        cmd.entity(root).add_child(child);
    }
}

/// Small files: street props. Vending machines, crates and balls.
pub fn spawn_prop(ctx: &mut SpawnCtx, f: &FileEntry, pos: Vec2, base_y: f32) {
    let seed = seed_for(&f.path);
    let mut rng = SmallRng::seed_from_u64(seed);
    let file_ref = FileRef {
        name: f.name.clone(),
        path: f.path.clone(),
        size: f.size,
        kind: f.kind,
    };
    let kind_mat = ctx.palette.body[&f.kind][rng.random_range(0..4usize)].clone();
    let choice = rng.random_range(0..10u32);

    if choice < 4 {
        // Vending machine with a glowing front.
        let root = ctx
            .commands
            .spawn((
                Mesh3d(ctx.meshes.cube.clone()),
                MeshMaterial3d(kind_mat),
                Transform::from_xyz(pos.x, base_y + 0.5, pos.y)
                    .with_rotation(Quat::from_rotation_y(rng.random_range(-0.3..0.3)))
                    .with_scale(Vec3::new(0.62, 1.0, 0.55)),
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Mass(16.0),
                // Explicit so the gravity-gun grab/carry systems can toggle it.
                GravityScale(1.0),
                Friction::new(0.8),
                Restitution::new(0.1),
                file_ref,
                Prop,
                BuildingBody,
            ))
            .id();
        let front = ctx
            .commands
            .spawn((
                Mesh3d(ctx.meshes.quad.clone()),
                MeshMaterial3d(ctx.palette.vend_front[(seed % 2) as usize].clone()),
                Transform::from_xyz(0.0, 0.06, 0.51).with_scale(Vec3::new(0.72, 0.75, 1.0)),
                NotShadowCaster,
            ))
            .id();
        ctx.commands.entity(root).add_child(front);
    } else if choice < 7 {
        let s = rng.random_range(0.36..0.52);
        ctx.commands.spawn((
            Mesh3d(ctx.meshes.cube.clone()),
            MeshMaterial3d(kind_mat),
            Transform::from_xyz(pos.x, base_y + 0.6, pos.y).with_scale(Vec3::splat(s)),
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Mass(2.0),
            GravityScale(1.0),
            Friction::new(0.7),
            Restitution::new(0.3),
            file_ref,
            Prop,
            BuildingBody,
        ));
    } else {
        let s = rng.random_range(0.3..0.45);
        ctx.commands.spawn((
            Mesh3d(ctx.meshes.sphere.clone()),
            MeshMaterial3d(kind_mat),
            Transform::from_xyz(pos.x, base_y + 0.6, pos.y).with_scale(Vec3::splat(s)),
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Mass(1.5),
            GravityScale(1.0),
            Friction::new(0.5),
            Restitution::new(0.5),
            file_ref,
            Prop,
            BuildingBody,
        ));
    }
}
