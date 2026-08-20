//! Turns the scanned `DirNode` tree into a physical city: districts laid out
//! with a squarified treemap, buildings per file, walls, slabs and signs.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::scan::{DirNode, FileEntry, FileKind};
use crate::{AppState, CityTree};

pub struct CityGenPlugin;

impl Plugin for CityGenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Playing), build_city);
    }
}

// ---------------------------------------------------------------------------
// Components & resources shared with the rest of the game
// ---------------------------------------------------------------------------

/// Any interactable object representing a real file.
#[derive(Component, Clone)]
pub struct FileRef {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub kind: FileKind,
}

/// Small dynamic object you can grab, carry, kick and throw.
#[derive(Component)]
pub struct Prop;

/// Panel on a text/code building awaiting a rendered-text texture.
#[derive(Component)]
pub struct TextScreen {
    pub path: PathBuf,
    pub kind: FileKind,
}

/// Panel on an image building awaiting the decoded image texture.
#[derive(Component)]
pub struct ImageScreen {
    pub path: PathBuf,
    /// Base panel dimensions (width, height) used to fit the image aspect.
    pub base_size: Vec2,
}

/// Floating district name plate awaiting a rendered-text texture.
#[derive(Component)]
pub struct SignText(pub String);

/// Decorative element that bobs up and down.
#[derive(Component)]
pub struct Bobber {
    pub base_y: f32,
    pub phase: f32,
    pub amp: f32,
}

#[derive(Clone)]
pub struct District {
    pub rect: Rect2,
    pub display_path: String,
    pub depth: usize,
}

#[derive(Resource, Default)]
pub struct Districts(pub Vec<District>);

/// Global facts about the generated city.
#[derive(Resource)]
pub struct CityMeta {
    pub spawn_pos: Vec3,
    pub half_extent: f32,
}

/// Shared meshes; everything is a scaled unit primitive so Bevy can batch.
#[derive(Resource)]
pub struct CityMeshes {
    pub cube: Handle<Mesh>,
    pub sphere: Handle<Mesh>,
    pub cylinder: Handle<Mesh>,
    pub quad: Handle<Mesh>,
}

/// Shared material palette, keyed by role.
#[derive(Resource)]
pub struct Palette {
    pub body: HashMap<FileKind, Handle<StandardMaterial>>,
    pub highlight: HashMap<FileKind, Handle<StandardMaterial>>,
    pub slabs: Vec<Handle<StandardMaterial>>,
    pub ground: Handle<StandardMaterial>,
    pub wall: Handle<StandardMaterial>,
    pub roof: Handle<StandardMaterial>,
    pub screen_off: Handle<StandardMaterial>,
    pub sign_bg: Handle<StandardMaterial>,
    pub orb: Handle<StandardMaterial>,
    pub marquee: Handle<StandardMaterial>,
    pub chest_trim: Handle<StandardMaterial>,
    pub eye: Handle<StandardMaterial>,
    pub projectile: Handle<StandardMaterial>,
}

pub fn kind_color(kind: FileKind) -> Color {
    match kind {
        FileKind::Image => Color::srgb(0.18, 0.77, 0.71),
        FileKind::Text => Color::srgb(0.35, 0.66, 0.90),
        FileKind::Code => Color::srgb(0.49, 0.44, 0.94),
        FileKind::Audio => Color::srgb(0.75, 0.36, 0.88),
        FileKind::Video => Color::srgb(1.00, 0.62, 0.26),
        FileKind::Archive => Color::srgb(0.69, 0.47, 0.23),
        FileKind::Executable => Color::srgb(0.55, 0.60, 0.68),
        FileKind::Data => Color::srgb(0.34, 0.80, 0.60),
        FileKind::Other => Color::srgb(0.60, 0.65, 0.70),
    }
}

// ---------------------------------------------------------------------------
// Treemap layout
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Rect2 {
    pub min: Vec2,
    pub size: Vec2,
}

impl Rect2 {
    pub fn center(&self) -> Vec2 {
        self.min + self.size * 0.5
    }
    pub fn max(&self) -> Vec2 {
        self.min + self.size
    }
    pub fn area(&self) -> f32 {
        self.size.x * self.size.y
    }
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.y >= self.min.y && p.x <= self.max().x && p.y <= self.max().y
    }
    pub fn inset(&self, by: f32) -> Rect2 {
        let by = by.min(self.size.x * 0.45).min(self.size.y * 0.45);
        Rect2 {
            min: self.min + Vec2::splat(by),
            size: self.size - Vec2::splat(by * 2.0),
        }
    }
}

/// Squarified treemap (Bruls, Huizing, van Wijk). Returns one rect per weight,
/// in the same order as the input.
pub fn squarify(weights: &[f32], rect: Rect2) -> Vec<Rect2> {
    let total: f32 = weights.iter().sum();
    let mut out = vec![
        Rect2 {
            min: rect.min,
            size: Vec2::ZERO
        };
        weights.len()
    ];
    if total <= 0.0 || weights.is_empty() {
        return out;
    }
    let scale = rect.area() / total;
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| weights[b].partial_cmp(&weights[a]).unwrap());
    let areas: Vec<f32> = order.iter().map(|&i| weights[i] * scale).collect();

    let mut remaining = rect;
    let mut row: Vec<usize> = Vec::new(); // indices into `order`/`areas`

    fn worst(row: &[usize], areas: &[f32], side: f32) -> f32 {
        let sum: f32 = row.iter().map(|&i| areas[i]).sum();
        if sum <= 0.0 || side <= 0.0 {
            return f32::INFINITY;
        }
        let mut w: f32 = 0.0;
        for &i in row {
            let a = areas[i].max(1e-6);
            let r1 = (side * side * a) / (sum * sum);
            let r2 = (sum * sum) / (side * side * a);
            w = w.max(r1.max(r2));
        }
        w
    }

    fn layout_row(
        row: &[usize],
        areas: &[f32],
        order: &[usize],
        remaining: &mut Rect2,
        out: &mut [Rect2],
    ) {
        let sum: f32 = row.iter().map(|&i| areas[i]).sum();
        if sum <= 0.0 {
            return;
        }
        let horizontal = remaining.size.x >= remaining.size.y;
        if horizontal {
            // Strip on the left, items stacked along y.
            let strip_w = (sum / remaining.size.y).min(remaining.size.x);
            let mut y = remaining.min.y;
            for &i in row {
                let h = areas[i] / strip_w.max(1e-6);
                out[order[i]] = Rect2 {
                    min: Vec2::new(remaining.min.x, y),
                    size: Vec2::new(strip_w, h),
                };
                y += h;
            }
            remaining.min.x += strip_w;
            remaining.size.x -= strip_w;
        } else {
            let strip_h = (sum / remaining.size.x).min(remaining.size.y);
            let mut x = remaining.min.x;
            for &i in row {
                let w = areas[i] / strip_h.max(1e-6);
                out[order[i]] = Rect2 {
                    min: Vec2::new(x, remaining.min.y),
                    size: Vec2::new(w, strip_h),
                };
                x += w;
            }
            remaining.min.y += strip_h;
            remaining.size.y -= strip_h;
        }
    }

    for i in 0..areas.len() {
        let side = remaining.size.x.min(remaining.size.y);
        let mut candidate = row.clone();
        candidate.push(i);
        if row.is_empty() || worst(&candidate, &areas, side) <= worst(&row, &areas, side) {
            row = candidate;
        } else {
            layout_row(&row, &areas, &order, &mut remaining, &mut out);
            row = vec![i];
        }
    }
    layout_row(&row, &areas, &order, &mut remaining, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Weights & dimensions
// ---------------------------------------------------------------------------

fn file_weight(f: &FileEntry) -> f32 {
    (3.0 + (f.size as f32 + 1.0).log2() * 0.75).clamp(3.0, 30.0)
}

fn dir_weight(d: &DirNode) -> f32 {
    let children: f32 = d.dirs.iter().map(dir_weight).sum::<f32>()
        + d.files.iter().map(file_weight).sum::<f32>();
    children * 1.22 + 10.0
}

fn road_width(depth: usize) -> f32 {
    match depth {
        0 => 4.6,
        1 => 3.0,
        2 => 2.1,
        _ => 1.5,
    }
}

fn slab_top(depth: usize) -> f32 {
    0.06 + depth as f32 * 0.05
}

fn seed_for(path: &std::path::Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// City construction
// ---------------------------------------------------------------------------

fn build_city(
    mut commands: Commands,
    tree: Res<CityTree>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let city_meshes = CityMeshes {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        sphere: meshes.add(Sphere::new(0.5)),
        cylinder: meshes.add(Cylinder::new(0.5, 1.0)),
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
    };

    let mut body = HashMap::new();
    let mut highlight = HashMap::new();
    for kind in [
        FileKind::Image,
        FileKind::Text,
        FileKind::Code,
        FileKind::Audio,
        FileKind::Video,
        FileKind::Archive,
        FileKind::Executable,
        FileKind::Data,
        FileKind::Other,
    ] {
        let color = kind_color(kind);
        body.insert(
            kind,
            materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: 0.85,
                ..default()
            }),
        );
        highlight.insert(
            kind,
            materials.add(StandardMaterial {
                base_color: color.lighter(0.12),
                emissive: LinearRgba::from(color) * 1.6,
                perceptual_roughness: 0.6,
                ..default()
            }),
        );
    }

    let slabs = (0..6)
        .map(|d| {
            materials.add(StandardMaterial {
                base_color: Color::hsl(210.0 - d as f32 * 38.0, 0.16, 0.66 + d as f32 * 0.03),
                perceptual_roughness: 0.95,
                ..default()
            })
        })
        .collect();

    let palette = Palette {
        body,
        highlight,
        slabs,
        ground: materials.add(StandardMaterial {
            base_color: Color::srgb(0.205, 0.22, 0.245),
            perceptual_roughness: 1.0,
            ..default()
        }),
        wall: materials.add(StandardMaterial {
            base_color: Color::srgb(0.78, 0.80, 0.84),
            perceptual_roughness: 0.9,
            ..default()
        }),
        roof: materials.add(StandardMaterial {
            base_color: Color::srgb(0.27, 0.30, 0.34),
            perceptual_roughness: 0.9,
            ..default()
        }),
        screen_off: materials.add(StandardMaterial {
            base_color: Color::srgb(0.05, 0.07, 0.10),
            emissive: LinearRgba::rgb(0.02, 0.05, 0.08),
            perceptual_roughness: 0.4,
            ..default()
        }),
        sign_bg: materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.12, 0.18),
            perceptual_roughness: 0.5,
            cull_mode: None,
            double_sided: true,
            ..default()
        }),
        orb: materials.add(StandardMaterial {
            base_color: Color::srgb(0.75, 0.36, 0.88),
            emissive: LinearRgba::rgb(1.8, 0.6, 2.4),
            perceptual_roughness: 0.3,
            ..default()
        }),
        marquee: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.62, 0.26),
            emissive: LinearRgba::rgb(2.6, 1.1, 0.25),
            ..default()
        }),
        chest_trim: materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.68, 0.25),
            metallic: 0.8,
            perceptual_roughness: 0.35,
            ..default()
        }),
        eye: materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 1.0, 1.0),
            emissive: LinearRgba::rgb(0.4, 3.0, 3.0),
            unlit: true,
            ..default()
        }),
        projectile: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.55, 0.15),
            emissive: LinearRgba::rgb(4.0, 1.6, 0.3),
            ..default()
        }),
    };

    // --- Root layout ------------------------------------------------------
    let root = &tree.0;
    let total_weight = dir_weight(root);
    let side = (total_weight * 10.0).sqrt().clamp(40.0, 460.0);
    let root_rect = Rect2 {
        min: Vec2::splat(-side * 0.5),
        size: Vec2::splat(side),
    };

    // Ground slab (extends past the city for a horizon).
    let ground_side = side + 120.0;
    commands.spawn((
        Mesh3d(city_meshes.cube.clone()),
        MeshMaterial3d(palette.ground.clone()),
        Transform::from_xyz(0.0, -0.3, 0.0).with_scale(Vec3::new(ground_side, 0.6, ground_side)),
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
    ));

    let mut districts = Districts::default();
    let mut ctx = SpawnCtx {
        commands: &mut commands,
        meshes: &city_meshes,
        palette: &palette,
        districts: &mut districts,
    };
    spawn_district(&mut ctx, root, root_rect, 0, root.name.clone());

    let spawn_pos = Vec3::new(0.0, 2.0, root_rect.max().y + 7.0);
    commands.insert_resource(CityMeta {
        spawn_pos,
        half_extent: side * 0.5,
    });
    commands.insert_resource(city_meshes);
    commands.insert_resource(palette);
    commands.insert_resource(districts);
}

struct SpawnCtx<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    meshes: &'a CityMeshes,
    palette: &'a Palette,
    districts: &'a mut Districts,
}

fn spawn_district(
    ctx: &mut SpawnCtx,
    node: &DirNode,
    rect: Rect2,
    depth: usize,
    display_path: String,
) {
    if rect.size.x < 2.0 || rect.size.y < 2.0 {
        return;
    }
    let top = slab_top(depth);
    let slab_mat = ctx.palette.slabs[depth.min(ctx.palette.slabs.len() - 1)].clone();
    let center = rect.center();

    ctx.commands.spawn((
        Mesh3d(ctx.meshes.cube.clone()),
        MeshMaterial3d(slab_mat),
        Transform::from_xyz(center.x, top * 0.5, center.y)
            .with_scale(Vec3::new(rect.size.x, top, rect.size.y)),
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
    ));

    ctx.districts.0.push(District {
        rect,
        display_path: display_path.clone(),
        depth,
    });

    // Perimeter walls with a gate on each side, for top-level districts only.
    if depth == 1 {
        spawn_walls(ctx, rect, top);
    }

    // Floating name sign on the south edge for shallow districts.
    if depth >= 1 && depth <= 2 && rect.size.x > 6.0 {
        ctx.commands.spawn((
            Mesh3d(ctx.meshes.quad.clone()),
            MeshMaterial3d(ctx.palette.sign_bg.clone()),
            Transform::from_xyz(center.x, top + 3.0, rect.max().y - 0.3)
                .with_scale(Vec3::new((rect.size.x * 0.5).clamp(3.6, 7.0), 1.1, 1.0)),
            SignText(node.name.clone()),
            NotShadowCaster,
            Bobber {
                base_y: top + 3.0,
                phase: center.x * 0.37,
                amp: 0.08,
            },
        ));
    }

    // Lay out children: subdirectories and files tile the same rectangle.
    let mut weights: Vec<f32> = Vec::new();
    for d in &node.dirs {
        weights.push(dir_weight(d));
    }
    for f in &node.files {
        weights.push(file_weight(f));
    }
    if weights.is_empty() {
        return;
    }
    let inner = rect.inset(road_width(depth) * 0.35);
    let cells = squarify(&weights, inner);
    let pad = road_width(depth + 1) * 0.5;

    for (i, d) in node.dirs.iter().enumerate() {
        let cell = cells[i].inset(pad);
        spawn_district(ctx, d, cell, depth + 1, format!("{display_path}/{}", d.name));
    }
    let base = node.dirs.len();
    for (i, f) in node.files.iter().enumerate() {
        let cell = cells[base + i].inset(0.5);
        spawn_file(ctx, f, cell, depth);
    }
}

fn spawn_walls(ctx: &mut SpawnCtx, rect: Rect2, top: f32) {
    let h = 1.1;
    let t = 0.25;
    let mn = rect.min;
    let mx = rect.max();
    // (start, end, along_x)
    let sides = [
        (Vec2::new(mn.x, mn.y), Vec2::new(mx.x, mn.y), true),
        (Vec2::new(mn.x, mx.y), Vec2::new(mx.x, mx.y), true),
        (Vec2::new(mn.x, mn.y), Vec2::new(mn.x, mx.y), false),
        (Vec2::new(mx.x, mn.y), Vec2::new(mx.x, mx.y), false),
    ];
    for (a, b, along_x) in sides {
        let len = a.distance(b);
        let gate = (len * 0.35).clamp(3.0, 7.0);
        let seg = (len - gate) * 0.5;
        if seg < 0.8 {
            continue;
        }
        for k in [0.0, 1.0] {
            // k=0: segment near `a`; k=1: segment near `b`.
            let t_center = if k == 0.0 {
                seg * 0.5
            } else {
                len - seg * 0.5
            };
            let pos = a + (b - a).normalize() * t_center;
            let (sx, sz) = if along_x { (seg, t) } else { (t, seg) };
            ctx.commands.spawn((
                Mesh3d(ctx.meshes.cube.clone()),
                MeshMaterial3d(ctx.palette.wall.clone()),
                Transform::from_xyz(pos.x, top + h * 0.5, pos.y)
                    .with_scale(Vec3::new(sx, h, sz)),
                RigidBody::Static,
                Collider::cuboid(1.0, 1.0, 1.0),
            ));
        }
    }
}

const PROP_SIZE_LIMIT: u64 = 4096;

fn spawn_file(ctx: &mut SpawnCtx, f: &FileEntry, cell: Rect2, depth: usize) {
    if cell.size.x < 0.9 || cell.size.y < 0.9 {
        return;
    }
    let mut rng = SmallRng::seed_from_u64(seed_for(&f.path));
    let base_y = slab_top(depth);
    let center = cell.center();
    let fp = (cell.size.x.min(cell.size.y) * 0.62).clamp(0.9, 6.5);
    let height = (1.6 + (f.size as f32 + 1.0).log2() * 0.55).clamp(2.0, 22.0);
    let file_ref = FileRef {
        name: f.name.clone(),
        path: f.path.clone(),
        size: f.size,
        kind: f.kind,
    };

    // Tiny files become physics props instead of buildings.
    if f.size < PROP_SIZE_LIMIT {
        let is_ball = rng.random_bool(0.4);
        let s = rng.random_range(0.38..0.55);
        let mut e = ctx.commands.spawn((
            Transform::from_xyz(center.x, base_y + 1.0, center.y).with_scale(Vec3::splat(s)),
            MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
            RigidBody::Dynamic,
            Mass(2.0),
            Friction::new(0.7),
            Restitution::new(0.3),
            file_ref,
            Prop,
        ));
        if is_ball {
            e.insert((Mesh3d(ctx.meshes.sphere.clone()), Collider::sphere(0.5)));
        } else {
            e.insert((
                Mesh3d(ctx.meshes.cube.clone()),
                Collider::cuboid(1.0, 1.0, 1.0),
            ));
        }
        return;
    }

    match f.kind {
        FileKind::Text | FileKind::Code => {
            let (w, d) = (fp * 0.62, fp * 0.4);
            let h = height * 0.9;
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, base_y + h * 0.5, center.y)
                        .with_scale(Vec3::new(w, h, d)),
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    // Scrolling text panel; note children inherit parent scale,
                    // so sizes here are in parent-local (scaled) space.
                    parent.spawn((
                        Mesh3d(ctx.meshes.quad.clone()),
                        MeshMaterial3d(ctx.palette.screen_off.clone()),
                        Transform::from_xyz(0.0, 0.02, 0.5 + 0.02 / d)
                            .with_scale(Vec3::new(0.82, 0.88, 1.0)),
                        TextScreen {
                            path: f.path.clone(),
                            kind: f.kind,
                        },
                        NotShadowCaster,
                    ));
                });
        }
        FileKind::Image => {
            let (w, h, d) = (fp * 1.0, (height * 0.62).clamp(2.0, 9.0), fp * 0.55);
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, base_y + h * 0.5, center.y)
                        .with_scale(Vec3::new(w, h, d)),
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(ctx.meshes.quad.clone()),
                        MeshMaterial3d(ctx.palette.screen_off.clone()),
                        Transform::from_xyz(0.0, 0.06, 0.5 + 0.02 / d)
                            .with_scale(Vec3::new(0.86, 0.72, 1.0)),
                        ImageScreen {
                            path: f.path.clone(),
                            base_size: Vec2::new(w * 0.86, h * 0.72),
                        },
                        NotShadowCaster,
                    ));
                });
        }
        FileKind::Audio => {
            let r = (fp * 0.44).clamp(0.5, 1.6);
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cylinder.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, base_y + 0.45, center.y)
                        .with_scale(Vec3::new(r * 2.0, 0.9, r * 2.0)),
                    RigidBody::Static,
                    Collider::cylinder(0.5, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    // Floating orb: local space is scaled, so apply the inverse
                    // of the parent scale to get a uniform world-space sphere.
                    let orb_d = (r * 1.1).clamp(0.7, 1.4);
                    let local_y = 1.7 / 0.9;
                    parent.spawn((
                        Mesh3d(ctx.meshes.sphere.clone()),
                        MeshMaterial3d(ctx.palette.orb.clone()),
                        Transform::from_xyz(0.0, local_y, 0.0).with_scale(Vec3::new(
                            orb_d / (r * 2.0),
                            orb_d / 0.9,
                            orb_d / (r * 2.0),
                        )),
                        NotShadowCaster,
                        Bobber {
                            base_y: local_y,
                            phase: center.x * 0.7 + center.y * 0.3,
                            amp: 0.25 / 0.9,
                        },
                    ));
                });
        }
        FileKind::Video => {
            let (w, h, d) = (fp * 1.05, (height * 0.55).clamp(2.2, 8.0), fp * 0.6);
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, base_y + h * 0.5, center.y)
                        .with_scale(Vec3::new(w, h, d)),
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(ctx.meshes.quad.clone()),
                        MeshMaterial3d(ctx.palette.screen_off.clone()),
                        Transform::from_xyz(0.0, 0.05, 0.5 + 0.02 / d)
                            .with_scale(Vec3::new(0.88, 0.6, 1.0)),
                        NotShadowCaster,
                    ));
                    // Glowing marquee bar on top.
                    parent.spawn((
                        Mesh3d(ctx.meshes.cube.clone()),
                        MeshMaterial3d(ctx.palette.marquee.clone()),
                        Transform::from_xyz(0.0, 0.5 + 0.09 / h, 0.0)
                            .with_scale(Vec3::new(1.06, 0.18 / h, 1.1)),
                        NotShadowCaster,
                    ));
                });
        }
        FileKind::Archive => {
            let (w, h, d) = (fp * 0.8, (fp * 0.5).clamp(0.8, 2.2), fp * 0.55);
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, base_y + h * 0.5, center.y)
                        .with_scale(Vec3::new(w, h, d)),
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    // Lid.
                    parent.spawn((
                        Mesh3d(ctx.meshes.cube.clone()),
                        MeshMaterial3d(ctx.palette.chest_trim.clone()),
                        Transform::from_xyz(0.0, 0.5, 0.0).with_scale(Vec3::new(1.06, 0.16, 1.08)),
                    ));
                    // Latch.
                    parent.spawn((
                        Mesh3d(ctx.meshes.cube.clone()),
                        MeshMaterial3d(ctx.palette.chest_trim.clone()),
                        Transform::from_xyz(0.0, 0.15, 0.5).with_scale(Vec3::new(0.14, 0.2, 0.08)),
                    ));
                });
        }
        FileKind::Executable => {
            let s = (fp * 0.5).clamp(0.7, 1.8);
            let body_h = s * 1.1;
            let body_y = base_y + s * 0.5 + body_h * 0.5;
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, body_y, center.y)
                        .with_scale(Vec3::new(s, body_h, s * 0.7)),
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.9, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    // Head.
                    parent.spawn((
                        Mesh3d(ctx.meshes.cube.clone()),
                        MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                        Transform::from_xyz(0.0, 0.5 + 0.36, 0.0)
                            .with_scale(Vec3::new(0.62, 0.55, 0.9)),
                    ));
                    // Eyes.
                    for ex in [-0.14, 0.14] {
                        parent.spawn((
                            Mesh3d(ctx.meshes.cube.clone()),
                            MeshMaterial3d(ctx.palette.eye.clone()),
                            Transform::from_xyz(ex, 0.5 + 0.4, 0.34)
                                .with_scale(Vec3::new(0.1, 0.1, 0.06)),
                            NotShadowCaster,
                        ));
                    }
                    // Legs.
                    for ex in [-0.28, 0.28] {
                        parent.spawn((
                            Mesh3d(ctx.meshes.cube.clone()),
                            MeshMaterial3d(ctx.palette.roof.clone()),
                            Transform::from_xyz(ex, -0.5 - 0.2, 0.0)
                                .with_scale(Vec3::new(0.2, 0.45, 0.5)),
                        ));
                    }
                });
        }
        FileKind::Data | FileKind::Other => {
            let (w, d) = (fp * 0.85, fp * 0.7);
            ctx.commands
                .spawn((
                    Mesh3d(ctx.meshes.cube.clone()),
                    MeshMaterial3d(ctx.palette.body[&f.kind].clone()),
                    Transform::from_xyz(center.x, base_y + height * 0.5, center.y)
                        .with_scale(Vec3::new(w, height, d)),
                    RigidBody::Static,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    file_ref.clone(),
                ))
                .with_children(|parent| {
                    parent.spawn((
                        Mesh3d(ctx.meshes.cube.clone()),
                        MeshMaterial3d(ctx.palette.roof.clone()),
                        Transform::from_xyz(0.0, 0.5 + 0.06 / height, 0.0)
                            .with_scale(Vec3::new(1.05, 0.12 / height, 1.05)),
                    ));
                });
        }
    }
}
