//! FPS-style interaction: crosshair raycast with highlight, inspect (E),
//! gravity-gun grab/carry/throw (F / click), and physics projectiles (click
//! with empty hands). Every file opens inside the game: text/code readers,
//! image viewers, audio playback, archive listings and hex dumps for
//! binaries. Files are never launched; the only outward action is R, which
//! reveals the file's location in Finder without opening the file itself.

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
        path: PathBuf,
        wrap: bool,
    },
    Image {
        title: String,
        path: PathBuf,
        meta: String,
    },
    Info {
        title: String,
        lines: Vec<String>,
        path: PathBuf,
    },
    /// Live preview rendered with the file's own typeface.
    Font {
        title: String,
        meta: String,
        path: PathBuf,
        font: Handle<Font>,
    },
}

impl InspectorContent {
    /// The real file this overlay is about (used by reveal-in-Finder).
    pub fn path(&self) -> &PathBuf {
        match self {
            InspectorContent::Text { path, .. }
            | InspectorContent::Image { path, .. }
            | InspectorContent::Info { path, .. }
            | InspectorContent::Font { path, .. } => path,
        }
    }
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
    mut fonts: ResMut<Assets<Font>>,
    mut props: Query<&mut GravityScale, With<Prop>>,
) {
    // E: open/close the inspector (or toggle audio).
    if keys.just_pressed(KeyCode::KeyE) {
        if inspector.0.is_some() {
            inspector.0 = None;
            grab.0 = true;
        } else if let Some(info) = hovered.0.as_ref().filter(|h| h.distance <= USE_RANGE) {
            let f = &info.file;
            if f.kind == FileKind::Audio {
                play_audio.write(PlayAudio {
                    path: f.path.clone(),
                    name: f.name.clone(),
                });
            }
            inspector.0 = Some(build_inspector_content(f, &mut fonts));
            grab.0 = false;
        }
    }

    // Esc: close the inspector and return to the game.
    if keys.just_pressed(KeyCode::Escape) && inspector.0.is_some() {
        inspector.0 = None;
        grab.0 = true;
    }

    // R: reveal the file's location in Finder (never opens the file).
    // While inspecting, reveals the inspected file; otherwise the hovered one.
    if keys.just_pressed(KeyCode::KeyR) {
        let path = match inspector.0.as_ref() {
            Some(content) => Some(content.path().clone()),
            None => hovered
                .0
                .as_ref()
                .filter(|h| h.distance <= USE_RANGE)
                .map(|h| h.file.path.clone()),
        };
        if let Some(path) = path {
            reveal_in_finder(&path);
        }
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

/// Chooses the richest in-game viewer for a file: extension-specific
/// handlers first (tables, pretty JSON, PDFs, fonts, gzip text), then a
/// kind-based fallback (reader, image viewer, archive listing, hex dump).
fn build_inspector_content(f: &FileRef, fonts: &mut Assets<Font>) -> InspectorContent {
    use crate::viewers;

    let meta = format!(
        "{} · {} · {}",
        f.kind.label(),
        crate::scan::human_size(f.size),
        f.path.display()
    );
    let ext = f
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let is_tarball = f.name.to_lowercase().ends_with(".tar.gz");

    let text = |title: String, body: String, wrap: bool| InspectorContent::Text {
        title,
        body,
        meta: meta.clone(),
        path: f.path.clone(),
        wrap,
    };

    match ext.as_str() {
        "csv" => text(
            format!("{} — table", f.name),
            viewers::csv_preview(&f.path, false),
            false,
        ),
        "tsv" => text(
            format!("{} — table", f.name),
            viewers::csv_preview(&f.path, true),
            false,
        ),
        "json" => text(f.name.clone(), viewers::json_preview(&f.path), true),
        "xml" | "plist" => text(f.name.clone(), viewers::xml_preview(&f.path), false),
        "rtf" => text(f.name.clone(), viewers::rtf_preview(&f.path), true),
        "pdf" => text(
            format!("{} — extracted text", f.name),
            viewers::pdf_preview(&f.path, f.size),
            true,
        ),
        "docx" => text(
            format!("{} — extracted text", f.name),
            viewers::docx_preview(&f.path, f.size),
            true,
        ),
        "xlsx" => text(
            format!("{} — spreadsheet", f.name),
            viewers::xlsx_preview(&f.path, f.size),
            false,
        ),
        "pptx" => text(
            format!("{} — slides", f.name),
            viewers::pptx_preview(&f.path, f.size),
            true,
        ),
        "epub" => text(
            format!("{} — extracted text", f.name),
            viewers::epub_preview(&f.path, f.size),
            true,
        ),
        "db" | "sqlite" | "sqlite3" => text(
            format!("{} — tables", f.name),
            viewers::sqlite_preview(&f.path, f.size),
            false,
        ),
        "heic" | "heif" => InspectorContent::Info {
            title: f.name.clone(),
            lines: vec![
                format!("Kind:  Image ({ext})"),
                format!("Size:  {}", crate::scan::human_size(f.size)),
                format!("Path:  {}", f.path.display()),
                String::new(),
                "HEIC/HEIF is not decoded in-game (needs a system codec).".into(),
                "Press R to reveal it in Finder.".into(),
            ],
            path: f.path.clone(),
        },
        "gz" if !is_tarball => text(f.name.clone(), viewers::gz_preview(&f.path), true),
        "ttf" | "otf" => match viewers::read_font_bytes(&f.path) {
            Ok(bytes) => InspectorContent::Font {
                title: format!("{} — live preview", f.name),
                meta,
                path: f.path.clone(),
                font: fonts.add(Font::from_bytes(bytes)),
            },
            Err(err) => text(
                format!("{} — hex", f.name),
                format!("Could not preview font: {err}\n\n{}", viewers::hex_dump(&f.path, 2048, f.size)),
                false,
            ),
        },
        _ => match f.kind {
            FileKind::Text | FileKind::Code => {
                text(f.name.clone(), viewers::read_text_preview(&f.path, 60 * 1024), true)
            }
            FileKind::Image => InspectorContent::Image {
                title: f.name.clone(),
                path: f.path.clone(),
                meta,
            },
            FileKind::Archive => text(
                format!("{} — contents", f.name),
                viewers::archive_listing(&f.path),
                false,
            ),
            FileKind::Video => InspectorContent::Info {
                title: f.name.clone(),
                lines: viewers::video_preview(&f.path, f.size),
                path: f.path.clone(),
            },
            FileKind::Audio => InspectorContent::Info {
                title: format!("{} — playing", f.name),
                lines: viewers::audio_preview(&f.path, f.size),
                path: f.path.clone(),
            },
            FileKind::Executable | FileKind::Data | FileKind::Other => text(
                format!("{} — hex", f.name),
                viewers::hex_dump(&f.path, 4096, f.size),
                false,
            ),
        },
    }
}

/// Shows the file in its enclosing folder without opening the file: Finder's
/// "reveal" on macOS, the parent directory in the file manager elsewhere.
fn reveal_in_finder(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    if let Err(err) = result {
        warn!("failed to reveal {}: {err}", path.display());
    }
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
