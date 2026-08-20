//! FPS-style interaction: crosshair raycast with highlight, inspect (E),
//! gravity-gun grab/carry/throw (F / click), and physics projectiles (click
//! with empty hands). Every file opens inside the game: text/code readers,
//! image viewers, audio playback, archive listings and hex dumps for
//! binaries. Nothing shells out to external apps.

use std::path::PathBuf;

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::buildings::BuildingBody;
use crate::citygen::{FileRef, CityMeshes, Palette, Prop};
use crate::player::{CursorGrabbed, Player, PlayerCamera};
use crate::scan::FileKind;
use crate::{AppState, GameSet};

const INTERACT_RANGE: f32 = 30.0;
const USE_RANGE: f32 = 14.0;
const GRAB_RANGE: f32 = 5.0;
const HOLD_DISTANCE: f32 = 2.4;
const THROW_SPEED: f32 = 17.0;
const PROJECTILE_SPEED: f32 = 30.0;
const MAX_PROJECTILES: usize = 48;

pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Hovered>()
            .init_resource::<Held>()
            .init_resource::<Inspector>()
            .add_message::<PlayAudio>()
            .add_systems(
                Update,
                (
                    hover_raycast,
                    apply_highlight,
                    interact_keys,
                    hold_carried_prop,
                    shoot_or_throw,
                    projectile_gc,
                )
                    .chain()
                    .in_set(GameSet::Interact)
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

/// What the crosshair is currently pointing at.
#[derive(Resource, Default)]
pub struct Hovered(pub Option<HoverInfo>);

pub struct HoverInfo {
    pub entity: Entity,
    pub distance: f32,
    pub file: FileRef,
    pub is_prop: bool,
}

/// Prop currently carried by the player.
#[derive(Resource, Default)]
pub struct Held(pub Option<Entity>);

/// Fullscreen inspector overlay content; rendered by the UI module.
#[derive(Resource, Default)]
pub struct Inspector(pub Option<InspectorContent>);

pub enum InspectorContent {
    Text {
        title: String,
        body: String,
        meta: String,
    },
    Image {
        title: String,
        path: PathBuf,
        meta: String,
    },
    Info {
        title: String,
        lines: Vec<String>,
    },
}

/// Request to toggle playback of an audio file (handled by filereps).
#[derive(Message)]
pub struct PlayAudio {
    pub path: PathBuf,
    pub name: String,
}

/// Remembers the shared material a mesh had before hover-highlighting.
#[derive(Component)]
struct OrigMaterial(Handle<StandardMaterial>);

#[derive(Component)]
struct Projectile {
    spawned_at: f32,
}

fn hover_raycast(
    spatial: SpatialQuery,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    player: Query<Entity, With<Player>>,
    held: Res<Held>,
    grab: Res<CursorGrabbed>,
    inspector: Res<Inspector>,
    file_refs: Query<(&FileRef, Has<Prop>)>,
    parents: Query<&ChildOf>,
    mut hovered: ResMut<Hovered>,
) {
    hovered.0 = None;
    if !grab.0 || inspector.0.is_some() {
        return;
    }
    let Ok(cam) = camera.single() else { return };
    let Ok(player_entity) = player.single() else {
        return;
    };
    let mut excluded = vec![player_entity];
    if let Some(held_entity) = held.0 {
        excluded.push(held_entity);
    }
    let filter = SpatialQueryFilter::default().with_excluded_entities(excluded);
    let Some(hit) = spatial.cast_ray(
        cam.translation(),
        cam.forward(),
        INTERACT_RANGE,
        true,
        &filter,
    ) else {
        return;
    };

    // The hit collider may be a decorative child; walk up to find the FileRef.
    let mut entity = hit.entity;
    for _ in 0..4 {
        if let Ok((file, is_prop)) = file_refs.get(entity) {
            hovered.0 = Some(HoverInfo {
                entity,
                distance: hit.distance,
                file: file.clone(),
                is_prop,
            });
            return;
        }
        match parents.get(entity) {
            Ok(child_of) => entity = child_of.0,
            Err(_) => return,
        }
    }
}

/// Buildings are multi-part: the root holds `FileRef` while the body meshes
/// are `BuildingBody` children (props carry both on one entity). Highlight
/// swaps materials on every body part of the hovered owner.
fn apply_highlight(
    mut commands: Commands,
    hovered: Res<Hovered>,
    palette: Option<Res<Palette>>,
    mut materials_q: Query<&mut MeshMaterial3d<StandardMaterial>>,
    body_parts: Query<(), With<BuildingBody>>,
    children_q: Query<&Children>,
    parents: Query<&ChildOf>,
    file_refs: Query<(), With<FileRef>>,
    orig: Query<(Entity, &OrigMaterial)>,
) {
    let Some(palette) = palette else { return };
    let hovered_entity = hovered.0.as_ref().map(|h| h.entity);

    // Restore parts whose owning FileRef entity is no longer hovered.
    for (entity, orig_mat) in &orig {
        let owner = if file_refs.contains(entity) {
            entity
        } else {
            parents.get(entity).map(|c| c.0).unwrap_or(entity)
        };
        if Some(owner) != hovered_entity {
            if let Ok(mut mat) = materials_q.get_mut(entity) {
                mat.0 = orig_mat.0.clone();
            }
            commands.entity(entity).remove::<OrigMaterial>();
        }
    }

    // Highlight every body part of the newly hovered owner.
    if let Some(info) = &hovered.0 {
        let highlight = palette.highlight[&info.file.kind].clone();
        let mut targets: Vec<Entity> = Vec::new();
        if body_parts.contains(info.entity) {
            targets.push(info.entity);
        }
        if let Ok(children) = children_q.get(info.entity) {
            targets.extend(children.iter().filter(|c| body_parts.contains(*c)));
        }
        for target in targets {
            if orig.contains(target) {
                continue;
            }
            if let Ok(mut mat) = materials_q.get_mut(target) {
                commands.entity(target).insert(OrigMaterial(mat.0.clone()));
                mat.0 = highlight.clone();
            }
        }
    }
}

fn interact_keys(
    keys: Res<ButtonInput<KeyCode>>,
    hovered: Res<Hovered>,
    mut held: ResMut<Held>,
    mut inspector: ResMut<Inspector>,
    mut grab: ResMut<CursorGrabbed>,
    mut play_audio: MessageWriter<PlayAudio>,
    mut props: Query<&mut GravityScale, With<Prop>>,
) {
    // E: open/close the inspector (or toggle audio).
    if keys.just_pressed(KeyCode::KeyE) {
        if inspector.0.is_some() {
            inspector.0 = None;
            grab.0 = true;
        } else if let Some(info) = hovered.0.as_ref().filter(|h| h.distance <= USE_RANGE) {
            let f = &info.file;
            let meta = format!(
                "{} · {}",
                f.kind.label(),
                crate::scan::human_size(f.size)
            );
            match f.kind {
                FileKind::Text | FileKind::Code => {
                    let body = read_text_preview(&f.path, 60 * 1024);
                    inspector.0 = Some(InspectorContent::Text {
                        title: f.name.clone(),
                        body,
                        meta: format!("{meta} · {}", f.path.display()),
                    });
                    grab.0 = false;
                }
                FileKind::Image => {
                    inspector.0 = Some(InspectorContent::Image {
                        title: f.name.clone(),
                        path: f.path.clone(),
                        meta: format!("{meta} · {}", f.path.display()),
                    });
                    grab.0 = false;
                }
                FileKind::Audio => {
                    play_audio.write(PlayAudio {
                        path: f.path.clone(),
                        name: f.name.clone(),
                    });
                }
                FileKind::Archive => {
                    inspector.0 = Some(InspectorContent::Text {
                        title: format!("{} — contents", f.name),
                        body: archive_listing(&f.path),
                        meta: format!("{meta} · {}", f.path.display()),
                    });
                    grab.0 = false;
                }
                FileKind::Video => {
                    inspector.0 = Some(InspectorContent::Info {
                        title: f.name.clone(),
                        lines: vec![
                            format!("Kind:  {}", f.kind.label()),
                            format!("Size:  {}", crate::scan::human_size(f.size)),
                            format!("Path:  {}", f.path.display()),
                            String::new(),
                            "Video decoding is not supported in-game.".into(),
                        ],
                    });
                    grab.0 = false;
                }
                FileKind::Executable | FileKind::Data | FileKind::Other => {
                    inspector.0 = Some(InspectorContent::Text {
                        title: format!("{} — hex", f.name),
                        body: hex_dump(&f.path, 4096, f.size),
                        meta: format!("{meta} · {}", f.path.display()),
                    });
                    grab.0 = false;
                }
            }
        }
    }

    // Esc: close the inspector and return to the game.
    if keys.just_pressed(KeyCode::Escape) && inspector.0.is_some() {
        inspector.0 = None;
        grab.0 = true;
    }

    // F: grab or drop a prop.
    if keys.just_pressed(KeyCode::KeyF) {
        if let Some(entity) = held.0.take() {
            if let Ok(mut gravity) = props.get_mut(entity) {
                gravity.0 = 1.0;
            }
        } else if let Some(info) = hovered
            .0
            .as_ref()
            .filter(|h| h.is_prop && h.distance <= GRAB_RANGE)
        {
            held.0 = Some(info.entity);
            if let Ok(mut gravity) = props.get_mut(info.entity) {
                gravity.0 = 0.0;
            }
        }
    }
}

fn read_text_preview(path: &std::path::Path, max_bytes: usize) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return "<could not read file>".into();
    };
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut handle = file.take(max_bytes as u64);
    if handle.read_to_end(&mut buf).is_err() {
        return "<could not read file>".into();
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if buf.len() >= max_bytes {
        text.push_str("\n\n… (truncated)");
    }
    text
}

/// Classic hex+ASCII dump of the file's first bytes.
fn hex_dump(path: &std::path::Path, max_bytes: usize, total_size: u64) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return "<could not read file>".into();
    };
    let mut buf = Vec::with_capacity(max_bytes);
    if file.take(max_bytes as u64).read_to_end(&mut buf).is_err() {
        return "<could not read file>".into();
    }
    let mut out = String::with_capacity(buf.len() * 4);
    for (i, chunk) in buf.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => out.push_str(&format!("{b:02x} ")),
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for b in chunk {
            out.push(if (0x20..0x7f).contains(b) { *b as char } else { '.' });
        }
        out.push_str("|\n");
    }
    if total_size > buf.len() as u64 {
        out.push_str(&format!(
            "\n… {} of {} shown",
            crate::scan::human_size(buf.len() as u64),
            crate::scan::human_size(total_size)
        ));
    }
    out
}

/// Lists the entries inside zip- and tar-family archives, entirely in-game.
fn archive_listing(path: &std::path::Path) -> String {
    const MAX_ENTRIES: usize = 400;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut entries: Vec<(String, u64)> = Vec::new();
    let mut more = 0usize;

    let result: Result<(), String> = (|| {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        match ext.as_str() {
            "zip" | "jar" | "whl" => {
                let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
                for i in 0..zip.len() {
                    let entry = zip.by_index_raw(i).map_err(|e| e.to_string())?;
                    if entries.len() < MAX_ENTRIES {
                        entries.push((entry.name().to_string(), entry.size()));
                    } else {
                        more += 1;
                    }
                }
                Ok(())
            }
            "tar" => {
                list_tar(file, &mut entries, &mut more, MAX_ENTRIES)
            }
            "tgz" | "crate" => {
                list_tar(flate2::read::GzDecoder::new(file), &mut entries, &mut more, MAX_ENTRIES)
            }
            "gz" if name.ends_with(".tar.gz") => {
                list_tar(flate2::read::GzDecoder::new(file), &mut entries, &mut more, MAX_ENTRIES)
            }
            _ => Err(format!("listing .{ext} archives is not supported")),
        }
    })();

    match result {
        Ok(()) => {
            let mut out = format!("{} entries\n\n", entries.len() + more);
            for (name, size) in &entries {
                out.push_str(&format!("{:>9}  {}\n", crate::scan::human_size(*size), name));
            }
            if more > 0 {
                out.push_str(&format!("\n… and {more} more"));
            }
            out
        }
        Err(err) => format!(
            "Could not list contents: {err}\n\nFalling back to hex view:\n\n{}",
            hex_dump(path, 1024, std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
        ),
    }
}

fn list_tar<R: std::io::Read>(
    reader: R,
    entries: &mut Vec<(String, u64)>,
    more: &mut usize,
    max: usize,
) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entries.len() < max {
            entries.push((
                entry.path().map_err(|e| e.to_string())?.display().to_string(),
                entry.size(),
            ));
        } else {
            *more += 1;
        }
    }
    Ok(())
}

fn hold_carried_prop(
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    mut held: ResMut<Held>,
    mut props: Query<
        (
            &Transform,
            &mut LinearVelocity,
            &mut AngularVelocity,
            &mut GravityScale,
        ),
        With<Prop>,
    >,
) {
    let Some(entity) = held.0 else { return };
    let Ok(cam) = camera.single() else { return };
    let Ok((transform, mut vel, mut ang, mut gravity)) = props.get_mut(entity) else {
        held.0 = None;
        return;
    };
    let target = cam.translation() + cam.forward() * HOLD_DISTANCE;
    let to_target = target - transform.translation;
    if to_target.length() > 6.0 {
        // Prop got wedged or blasted away; let go.
        gravity.0 = 1.0;
        held.0 = None;
        return;
    }
    vel.0 = (to_target * 10.0).clamp_length_max(24.0);
    ang.0 *= 0.85;
}

fn shoot_or_throw(
    mut commands: Commands,
    mouse: Res<ButtonInput<MouseButton>>,
    grab: Res<CursorGrabbed>,
    inspector: Res<Inspector>,
    camera: Query<&GlobalTransform, With<PlayerCamera>>,
    mut held: ResMut<Held>,
    mut props: Query<(&mut LinearVelocity, &mut GravityScale), With<Prop>>,
    meshes: Option<Res<CityMeshes>>,
    palette: Option<Res<Palette>>,
    projectiles: Query<(Entity, &Projectile)>,
    time: Res<Time>,
) {
    if !grab.0 || inspector.0.is_some() || !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Ok(cam) = camera.single() else { return };
    let forward = cam.forward();

    // Throw the carried prop if we have one.
    if let Some(entity) = held.0.take() {
        if let Ok((mut vel, mut gravity)) = props.get_mut(entity) {
            gravity.0 = 1.0;
            vel.0 = forward * THROW_SPEED + Vec3::Y * 1.5;
        }
        return;
    }

    // Otherwise fire a glowing projectile.
    let (Some(meshes), Some(palette)) = (meshes, palette) else {
        return;
    };
    if projectiles.iter().count() >= MAX_PROJECTILES {
        // Recycle the oldest one.
        if let Some((oldest, _)) = projectiles
            .iter()
            .min_by(|a, b| a.1.spawned_at.total_cmp(&b.1.spawned_at))
        {
            commands.entity(oldest).despawn();
        }
    }
    commands.spawn((
        Mesh3d(meshes.sphere.clone()),
        MeshMaterial3d(palette.projectile.clone()),
        Transform::from_translation(cam.translation() + forward * 0.7)
            .with_scale(Vec3::splat(0.26)),
        RigidBody::Dynamic,
        Collider::sphere(0.5),
        Mass(0.6),
        Friction::new(0.5),
        Restitution::new(0.55),
        LinearVelocity(forward * PROJECTILE_SPEED),
        Projectile {
            spawned_at: time.elapsed_secs(),
        },
    ));
}

fn projectile_gc(
    mut commands: Commands,
    projectiles: Query<(Entity, &Projectile, &Transform)>,
    time: Res<Time>,
) {
    for (entity, projectile, transform) in &projectiles {
        if time.elapsed_secs() - projectile.spawned_at > 10.0 || transform.translation.y < -25.0 {
            commands.entity(entity).despawn();
        }
    }
}
