//! Turns the scanned `DirNode` tree into a dense, Japanese-style city.
//! Districts come from a squarified treemap; inside each district, files
//! become buildings packed shoulder-to-shoulder along the streets (perimeter
//! rows) and back alleys (interior rows), with hash-seeded variety.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::buildings::{spawn_building, spawn_prop};
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

/// Vertical neon shop sign showing the file name (rendered on approach).
#[derive(Component)]
pub struct NeonNameSign {
    pub name: String,
    pub hue_seed: u64,
}

/// Red aircraft-warning light on tall rooftops (pulsed at night).
#[derive(Component)]
pub struct Beacon;

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

/// Street-corner points (perimeter alley gaps) used for lamps and crosswalks.
#[derive(Resource, Default)]
pub struct Gates(pub Vec<Vec3>);

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

pub const NEON_COLORS: [Color; 6] = [
    Color::srgb(1.0, 0.18, 0.53),  // pink
    Color::srgb(0.0, 0.90, 1.0),   // cyan
    Color::srgb(0.62, 0.30, 1.0),  // purple
    Color::srgb(1.0, 0.55, 0.12),  // orange
    Color::srgb(0.25, 1.0, 0.55),  // green
    Color::srgb(1.0, 0.85, 0.25),  // yellow
];

/// Shared material palette, keyed by role.
#[derive(Resource)]
pub struct Palette {
    /// Four concrete-tinted shades per file kind.
    pub body: HashMap<FileKind, [Handle<StandardMaterial>; 4]>,
    pub highlight: HashMap<FileKind, Handle<StandardMaterial>>,
    pub slab: Handle<StandardMaterial>,
    pub sidewalk: Handle<StandardMaterial>,
    pub ground: Handle<StandardMaterial>,
    pub roof: Handle<StandardMaterial>,
    pub dark_metal: Handle<StandardMaterial>,
    pub gold_trim: Handle<StandardMaterial>,
    pub screen_off: Handle<StandardMaterial>,
    pub sign_bg: Handle<StandardMaterial>,
    pub orb: Handle<StandardMaterial>,
    pub marquee: Handle<StandardMaterial>,
    pub eye: Handle<StandardMaterial>,
    pub projectile: Handle<StandardMaterial>,
    pub neon: [Handle<StandardMaterial>; 6],
    pub neon_flicker: Handle<StandardMaterial>,
    pub window_lit: Handle<StandardMaterial>,
    pub window_dark: Handle<StandardMaterial>,
    pub awning: [Handle<StandardMaterial>; 2],
    pub vend_front: [Handle<StandardMaterial>; 2],
    pub beacon: Handle<StandardMaterial>,
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

/// Buildings read as concrete tinted toward the kind color, in four shades.
fn body_color(kind: FileKind, shade: usize) -> Color {
    let base = kind_color(kind).to_srgba();
    let gray = 0.42;
    let mix = 0.38;
    let s = [0.62, 0.82, 1.0, 1.22][shade];
    Color::srgb(
        (gray * (1.0 - mix) + base.red * mix) * s,
        (gray * (1.0 - mix) + base.green * mix) * s,
        (gray * (1.0 - mix) + base.blue * mix) * s,
    )
}

pub fn seed_for(path: &std::path::Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
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
    let mut row: Vec<usize> = Vec::new();

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
    (3.0 + (f.size as f32 + 1.0).log2() * 0.6).clamp(3.0, 24.0)
}

fn dir_weight(d: &DirNode) -> f32 {
    let children: f32 = d.dirs.iter().map(dir_weight).sum::<f32>()
        + d.files.iter().map(file_weight).sum::<f32>();
    children * 1.18 + 12.0
}

fn road_width(depth: usize) -> f32 {
    match depth {
        0 => 7.0,
        1 => 5.0,
        2 => 3.6,
        _ => 2.8,
    }
}

pub const SLAB_TOP: f32 = 0.12;
const ROW_DEPTH: f32 = 6.0;
const PROP_SIZE_LIMIT: u64 = 4096;

// ---------------------------------------------------------------------------
// City construction
// ---------------------------------------------------------------------------

pub fn build_city(
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
    let palette = make_palette(&mut materials);

    // --- Root layout ------------------------------------------------------
    let root = &tree.0;
    let total_weight = dir_weight(root);
    let side = (total_weight * 7.0).sqrt().clamp(40.0, 420.0);
    let root_rect = Rect2 {
        min: Vec2::splat(-side * 0.5),
        size: Vec2::splat(side),
    };

    // Asphalt ground extending past the city.
    let ground_side = side + 140.0;
    commands.spawn((
        Mesh3d(city_meshes.cube.clone()),
        MeshMaterial3d(palette.ground.clone()),
        Transform::from_xyz(0.0, -0.3, 0.0).with_scale(Vec3::new(ground_side, 0.6, ground_side)),
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
    ));

    let mut districts = Districts::default();
    let mut gates = Gates::default();
    let mut ctx = SpawnCtx {
        commands: &mut commands,
        meshes: &city_meshes,
        palette: &palette,
        districts: &mut districts,
        gates: &mut gates,
        building_count: 0,
    };
    spawn_district(&mut ctx, root, root_rect, 0, root.name.clone());
    info!(
        "city built: {} districts, {} buildings, side {:.0}m",
        ctx.districts.0.len(),
        ctx.building_count,
        side
    );

    let spawn_pos = Vec3::new(0.0, 2.0, root_rect.max().y + 6.0);
    commands.insert_resource(CityMeta {
        spawn_pos,
        half_extent: side * 0.5,
    });
    commands.insert_resource(city_meshes);
    commands.insert_resource(palette);
    commands.insert_resource(districts);
    commands.insert_resource(gates);
}

fn make_palette(materials: &mut Assets<StandardMaterial>) -> Palette {
    let concrete = |color: Color, rough: f32| StandardMaterial {
        base_color: color,
        perceptual_roughness: rough,
        ..default()
    };
    let neon = |color: Color| {
        let l = LinearRgba::from(color);
        StandardMaterial {
            base_color: color,
            emissive: l * 5.0,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            ..default()
        }
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
        body.insert(
            kind,
            [0, 1, 2, 3].map(|s| materials.add(concrete(body_color(kind, s), 0.92))),
        );
        let color = kind_color(kind);
        highlight.insert(
            kind,
            materials.add(StandardMaterial {
                base_color: color.lighter(0.10),
                emissive: LinearRgba::from(color) * 1.8,
                perceptual_roughness: 0.6,
                ..default()
            }),
        );
    }

    Palette {
        body,
        highlight,
        slab: materials.add(concrete(Color::srgb(0.30, 0.31, 0.35), 0.95)),
        sidewalk: materials.add(concrete(Color::srgb(0.42, 0.43, 0.47), 0.9)),
        // Wet-look asphalt: dark and fairly smooth so lights catch on it.
        ground: materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.105, 0.125),
            perceptual_roughness: 0.35,
            metallic: 0.05,
            ..default()
        }),
        roof: materials.add(concrete(Color::srgb(0.22, 0.235, 0.27), 0.95)),
        dark_metal: materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.17, 0.20),
            metallic: 0.6,
            perceptual_roughness: 0.5,
            ..default()
        }),
        gold_trim: materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.68, 0.25),
            metallic: 0.8,
            perceptual_roughness: 0.35,
            ..default()
        }),
        screen_off: materials.add(StandardMaterial {
            base_color: Color::srgb(0.04, 0.05, 0.08),
            emissive: LinearRgba::rgb(0.02, 0.04, 0.07),
            perceptual_roughness: 0.3,
            ..default()
        }),
        sign_bg: materials.add(StandardMaterial {
            base_color: Color::srgb(0.08, 0.09, 0.14),
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
        neon: NEON_COLORS.map(|c| materials.add(neon(c))),
        neon_flicker: materials.add(neon(Color::srgb(1.0, 0.25, 0.45))),
        window_lit: materials.add(StandardMaterial {
            base_color: Color::srgb(0.25, 0.22, 0.16),
            emissive: LinearRgba::rgb(1.4, 1.05, 0.55),
            ..default()
        }),
        window_dark: materials.add(StandardMaterial {
            base_color: Color::srgb(0.10, 0.11, 0.15),
            perceptual_roughness: 0.25,
            metallic: 0.3,
            ..default()
        }),
        awning: [
            materials.add(concrete(Color::srgb(0.55, 0.16, 0.18), 0.8)),
            materials.add(concrete(Color::srgb(0.14, 0.32, 0.45), 0.8)),
        ],
        vend_front: [
            materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.95, 1.0),
                emissive: LinearRgba::rgb(0.9, 1.3, 1.6),
                unlit: true,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.4, 0.35),
                emissive: LinearRgba::rgb(1.6, 0.5, 0.4),
                unlit: true,
                ..default()
            }),
        ],
        beacon: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.1, 0.1),
            emissive: LinearRgba::rgb(4.0, 0.2, 0.2),
            unlit: true,
            ..default()
        }),
    }
}

pub struct SpawnCtx<'a, 'w, 's> {
    pub commands: &'a mut Commands<'w, 's>,
    pub meshes: &'a CityMeshes,
    pub palette: &'a Palette,
    pub districts: &'a mut Districts,
    pub gates: &'a mut Gates,
    pub building_count: usize,
}

fn spawn_district(
    ctx: &mut SpawnCtx,
    node: &DirNode,
    rect: Rect2,
    depth: usize,
    display_path: String,
) {
    if rect.size.x < 4.0 || rect.size.y < 4.0 {
        return;
    }
    let center = rect.center();

    // Curb-height slab with a lighter sidewalk ring on top.
    ctx.commands.spawn((
        Mesh3d(ctx.meshes.cube.clone()),
        MeshMaterial3d(ctx.palette.slab.clone()),
        Transform::from_xyz(center.x, SLAB_TOP * 0.5, center.y)
            .with_scale(Vec3::new(rect.size.x, SLAB_TOP, rect.size.y)),
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
    ));
    spawn_sidewalk_ring(ctx, rect);

    ctx.districts.0.push(District {
        rect,
        display_path: display_path.clone(),
        depth,
    });

    // Hanging district name sign over the south gate.
    if depth >= 1 && depth <= 2 && rect.size.x > 8.0 {
        let sign_w = (rect.size.x * 0.5).clamp(5.0, 11.0);
        let sign_y = SLAB_TOP + 4.6;
        ctx.commands.spawn((
            Mesh3d(ctx.meshes.quad.clone()),
            MeshMaterial3d(ctx.palette.sign_bg.clone()),
            Transform::from_xyz(center.x, sign_y, rect.max().y + 0.6)
                .with_scale(Vec3::new(sign_w, sign_w * 0.22, 1.0)),
            SignText(node.name.clone()),
            NotShadowCaster,
            Bobber {
                base_y: sign_y,
                phase: center.x * 0.37,
                amp: 0.1,
            },
        ));
    }

    // Split files: props scatter on the streets, the rest become buildings.
    let mut rng = SmallRng::seed_from_u64(seed_for(&node.path));
    let mut buildings: Vec<&FileEntry> = Vec::new();
    let mut props: Vec<&FileEntry> = Vec::new();
    for f in &node.files {
        if f.size < PROP_SIZE_LIMIT {
            props.push(f);
        } else {
            buildings.push(f);
        }
    }
    buildings.sort_by(|a, b| b.size.cmp(&a.size));

    // Fill the street-facing perimeter row first (biggest files up front).
    let leftovers = if rect.size.x > 20.0 && rect.size.y > 20.0 {
        spawn_perimeter_row(ctx, rect, &buildings, &mut rng)
    } else {
        buildings.clone()
    };

    // Interior: subdistricts plus leftover-file alley blocks.
    let inner = rect.inset(if rect.size.x > 20.0 { ROW_DEPTH + 2.2 } else { 1.5 });
    let blocks: Vec<Vec<&FileEntry>> = leftovers.chunks(8).map(|c| c.to_vec()).collect();
    let mut weights: Vec<f32> = node.dirs.iter().map(dir_weight).collect();
    for b in &blocks {
        weights.push(b.iter().map(|f| file_weight(f)).sum::<f32>() * 1.6 + 6.0);
    }
    if !weights.is_empty() && inner.size.x > 6.0 && inner.size.y > 6.0 {
        let cells = squarify(&weights, inner);
        let pad = road_width(depth + 1) * 0.5;
        for (i, d) in node.dirs.iter().enumerate() {
            let cell = cells[i].inset(pad);
            spawn_district(ctx, d, cell, depth + 1, format!("{display_path}/{}", d.name));
        }
        for (bi, block) in blocks.iter().enumerate() {
            let cell = cells[node.dirs.len() + bi].inset(1.2);
            spawn_alley_block(ctx, block, cell, &mut rng);
        }
    }

    // Street props (vending machines, crates, balls) along the south edge.
    let mut px = rect.min.x + 3.0;
    for f in props {
        if px > rect.max().x - 3.0 {
            break;
        }
        spawn_prop(ctx, f, Vec2::new(px, rect.max().y - 1.3), SLAB_TOP);
        px += 2.2 + (seed_for(&f.path) % 30) as f32 * 0.1;
    }
}

fn spawn_sidewalk_ring(ctx: &mut SpawnCtx, rect: Rect2) {
    let w = 1.3;
    let y = SLAB_TOP + 0.006;
    let c = rect.center();
    let strips = [
        // (center, scale)
        (
            Vec2::new(c.x, rect.min.y + w * 0.5),
            Vec2::new(rect.size.x, w),
        ),
        (
            Vec2::new(c.x, rect.max().y - w * 0.5),
            Vec2::new(rect.size.x, w),
        ),
        (
            Vec2::new(rect.min.x + w * 0.5, c.y),
            Vec2::new(w, rect.size.y - w * 2.0),
        ),
        (
            Vec2::new(rect.max().x - w * 0.5, c.y),
            Vec2::new(w, rect.size.y - w * 2.0),
        ),
    ];
    for (pos, scale) in strips {
        ctx.commands.spawn((
            Mesh3d(ctx.meshes.quad.clone()),
            MeshMaterial3d(ctx.palette.sidewalk.clone()),
            Transform::from_xyz(pos.x, y, pos.y)
                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::new(scale.x, scale.y, 1.0)),
            NotShadowCaster,
        ));
    }
}

/// Packs buildings shoulder-to-shoulder along the district edge, facing the
/// street outside. Leaves alley gaps for walking in. Returns unplaced files.
fn spawn_perimeter_row<'f>(
    ctx: &mut SpawnCtx,
    rect: Rect2,
    files: &[&'f FileEntry],
    rng: &mut SmallRng,
) -> Vec<&'f FileEntry> {
    let corner = 3.2;
    let mn = rect.min;
    let mx = rect.max();
    // (start, along, outward normal, length): south, north, west, east.
    let sides = [
        (
            Vec2::new(mn.x + corner, mx.y),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
            rect.size.x - corner * 2.0,
        ),
        (
            Vec2::new(mx.x - corner, mn.y),
            Vec2::new(-1.0, 0.0),
            Vec2::new(0.0, -1.0),
            rect.size.x - corner * 2.0,
        ),
        (
            Vec2::new(mn.x, mn.y + corner),
            Vec2::new(0.0, 1.0),
            Vec2::new(-1.0, 0.0),
            rect.size.y - corner * 2.0,
        ),
        (
            Vec2::new(mx.x, mx.y - corner),
            Vec2::new(0.0, -1.0),
            Vec2::new(1.0, 0.0),
            rect.size.y - corner * 2.0,
        ),
    ];

    let mut queue = files.iter().copied().collect::<std::collections::VecDeque<_>>();
    for (start, along, out, length) in sides {
        if queue.is_empty() {
            break;
        }
        let mut cursor = 0.0;
        let mut since_gap = 0.0;
        while let Some(f) = queue.front() {
            let frng_seed = seed_for(&f.path);
            let mut frng = SmallRng::seed_from_u64(frng_seed);
            let w = (2.4 + frng.random_range(0.0..2.0) + (f.size as f32).log2() * 0.12)
                .clamp(2.4, 7.0);
            if cursor + w > length {
                break;
            }
            // Periodic alley gap into the district interior.
            if since_gap > 20.0 {
                let gap = 2.6;
                let gate = start + along * (cursor + gap * 0.5);
                ctx.gates.0.push(Vec3::new(gate.x, SLAB_TOP, gate.y));
                cursor += gap;
                since_gap = 0.0;
                if cursor + w > length {
                    break;
                }
            }
            let f = queue.pop_front().unwrap();
            let setback = frng.random_range(0.0..0.9);
            let bdepth = ROW_DEPTH - 1.0 - frng.random_range(0.0..0.8);
            // Building center: pulled inward from the street edge.
            let along_pos = start + along * (cursor + w * 0.5);
            let center = along_pos - out * (setback + bdepth * 0.5 + 0.2);
            // Local +Z must point along `out` (toward the street).
            let yaw = out.x.atan2(out.y);
            spawn_building(
                ctx,
                f,
                Vec3::new(center.x, SLAB_TOP, center.y),
                w,
                bdepth,
                yaw,
                &mut frng,
            );
            ctx.building_count += 1;
            let gap = if rng.random_bool(0.55) {
                0.0
            } else {
                rng.random_range(0.25..0.9)
            };
            cursor += w + gap;
            since_gap += w + gap;
        }
    }
    queue.into_iter().collect()
}

/// Leftover files form a dense row (or two back-to-back) inside a cell,
/// making narrow back alleys.
fn spawn_alley_block(ctx: &mut SpawnCtx, files: &[&FileEntry], cell: Rect2, rng: &mut SmallRng) {
    if cell.size.x < 4.0 || cell.size.y < 4.0 {
        return;
    }
    let horizontal = cell.size.x >= cell.size.y;
    let length = if horizontal { cell.size.x } else { cell.size.y };
    let depth_avail = if horizontal { cell.size.y } else { cell.size.x };
    let two_rows = depth_avail > ROW_DEPTH * 2.0 + 1.0;
    let c = cell.center();

    let rows: Vec<(Vec2, Vec2, Vec2)> = if two_rows {
        let off = depth_avail * 0.25;
        if horizontal {
            vec![
                (Vec2::new(cell.min.x, c.y - off), Vec2::X, Vec2::new(0.0, -1.0)),
                (Vec2::new(cell.min.x, c.y + off), Vec2::X, Vec2::new(0.0, 1.0)),
            ]
        } else {
            vec![
                (Vec2::new(c.x - off, cell.min.y), Vec2::Y, Vec2::new(-1.0, 0.0)),
                (Vec2::new(c.x + off, cell.min.y), Vec2::Y, Vec2::new(1.0, 0.0)),
            ]
        }
    } else if horizontal {
        vec![(Vec2::new(cell.min.x, c.y), Vec2::X, Vec2::new(0.0, 1.0))]
    } else {
        vec![(Vec2::new(c.x, cell.min.y), Vec2::Y, Vec2::new(1.0, 0.0))]
    };

    let mut queue = files.iter().copied().collect::<std::collections::VecDeque<_>>();
    for (row_origin, along, mut out) in rows {
        let mut cursor = 1.0;
        while let Some(f) = queue.front() {
            let mut frng = SmallRng::seed_from_u64(seed_for(&f.path));
            let w = (2.2 + frng.random_range(0.0..1.8) + (f.size as f32).log2() * 0.1)
                .clamp(2.2, 6.0);
            if cursor + w > length - 1.0 {
                break;
            }
            let f = queue.pop_front().unwrap();
            // Single rows: buildings face random directions for variety.
            if !two_rows && rng.random_bool(0.5) {
                out = -out;
            }
            let bdepth = (depth_avail * if two_rows { 0.42 } else { 0.7 })
                .clamp(2.5, ROW_DEPTH)
                - frng.random_range(0.0..0.5);
            let center = row_origin + along * (cursor + w * 0.5);
            let yaw = out.x.atan2(out.y);
            spawn_building(
                ctx,
                f,
                Vec3::new(center.x, SLAB_TOP, center.y),
                w,
                bdepth,
                yaw,
                &mut frng,
            );
            ctx.building_count += 1;
            cursor += w + rng.random_range(0.0..0.6);
        }
    }
}
