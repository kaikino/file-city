//! Brings buildings to life: renders real file contents onto text obelisk
//! screens (render-to-texture, then UV-scrolled), decodes images onto
//! building-front displays, plays audio files, and runs idle animations.
//! Screens activate near the player and are freed when far away.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::sprite::Anchor;
use bevy::tasks::{block_on, poll_once, AsyncComputeTaskPool, Task};
use bevy::text::TextBounds;

use crate::citygen::{Bobber, ImageScreen, Palette, SignText, TextScreen};
use crate::interact::{Inspector, InspectorContent, PlayAudio};
use crate::player::Player;
use crate::scan::FileKind;
use crate::AppState;

const RTT_LAYER: usize = 31;
const TEXT_SCREEN_BUDGET: usize = 20;
const IMAGE_SCREEN_BUDGET: usize = 30;
const SCREEN_ACTIVATE_DIST: f32 = 50.0;
const SIGN_ACTIVATE_DIST: f32 = 140.0;
const SCREEN_DEACTIVATE_DIST: f32 = 65.0;
const MAX_AUDIO_BYTES: u64 = 40 * 1024 * 1024;

pub struct FileRepsPlugin;

impl Plugin for FileRepsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RttQueue>()
            .init_resource::<RttActive>()
            .init_resource::<ScreenBudget>()
            .init_resource::<ImageCache>()
            .init_resource::<ImageTasks>()
            .init_resource::<CurrentAudio>()
            .init_resource::<AudioTask>()
            .add_message::<RequestImage>()
            .add_systems(
                Update,
                (
                    activate_screens,
                    process_rtt_queue,
                    handle_image_requests,
                    poll_image_tasks,
                    deactivate_far_screens,
                    scroll_text_screens,
                    handle_play_audio,
                    poll_audio_task,
                    animate_bobbers,
                    pulse_orbs,
                )
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Screens currently glowing, per category (bounds VRAM usage).
#[derive(Resource, Default)]
struct ScreenBudget {
    text: usize,
    images: usize,
}

/// Decoded image textures by file path, reused by the inspector overlay.
#[derive(Resource, Default)]
pub struct ImageCache(pub HashMap<PathBuf, Handle<Image>>);

/// Ask filereps to decode an image that has no nearby screen (inspector use).
#[derive(Message)]
pub struct RequestImage(pub PathBuf);

/// Marks a screen panel whose texture job is queued or in flight.
#[derive(Component)]
struct PendingScreen;

/// A panel that received its texture. Holds the handles so they can be freed.
#[derive(Component)]
struct ScreenReady {
    image: Handle<Image>,
    material: Handle<StandardMaterial>,
    is_text: bool,
}

/// UV-scroll animation for text marquees; the f32 is a per-screen phase.
#[derive(Component)]
struct ScrollText(f32);

/// Image panels that already adjusted their scale to the image aspect.
#[derive(Component)]
struct Fitted;

/// Panels whose content could not be produced (e.g. unsupported image
/// format); left dark and never retried.
#[derive(Component)]
struct ScreenFailed;

// ---------------------------------------------------------------------------
// Render-to-texture text pipeline
// ---------------------------------------------------------------------------

struct RttJob {
    /// Panel to apply the finished texture to; `None` renders nothing (unused).
    target: Entity,
    text: String,
    width: u32,
    height: u32,
    font_size: f32,
    fg: Color,
    bg: Color,
    /// Marquee screens scroll and glow harder; signs are static.
    scroll: bool,
    double_sided: bool,
}

#[derive(Resource, Default)]
struct RttQueue(VecDeque<RttJob>);

struct ActiveRtt {
    job: RttJob,
    camera: Entity,
    text_entity: Entity,
    image: Handle<Image>,
    frames_left: u8,
}

#[derive(Resource, Default)]
struct RttActive(Option<ActiveRtt>);

fn read_prefix(path: &Path, max_bytes: usize) -> String {
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buf = Vec::new();
    if file.take(max_bytes as u64).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf)
        .replace('\r', "")
        .replace('\t', "  ")
}

/// Picks nearby unlit screens and queues their content, within budget.
fn activate_screens(
    mut commands: Commands,
    player: Query<&Transform, With<Player>>,
    text_screens: Query<
        (Entity, &GlobalTransform, &TextScreen),
        (Without<PendingScreen>, Without<ScreenReady>, Without<ScreenFailed>),
    >,
    image_screens: Query<
        (Entity, &GlobalTransform, &ImageScreen),
        (Without<PendingScreen>, Without<ScreenReady>, Without<ScreenFailed>),
    >,
    signs: Query<
        (Entity, &GlobalTransform, &SignText),
        (Without<PendingScreen>, Without<ScreenReady>, Without<ScreenFailed>),
    >,
    mut budget: ResMut<ScreenBudget>,
    mut queue: ResMut<RttQueue>,
    mut tasks: ResMut<ImageTasks>,
    time: Res<Time>,
    mut last_run: Local<f32>,
) {
    // This scan is cheap but doesn't need to happen every frame.
    if time.elapsed_secs() - *last_run < 0.3 {
        return;
    }
    *last_run = time.elapsed_secs();
    let Ok(player_pos) = player.single().map(|t| t.translation) else {
        return;
    };

    // District signs: persistent, generated once when first approached.
    for (entity, transform, sign) in &signs {
        if transform.translation().distance(player_pos) > SIGN_ACTIVATE_DIST {
            continue;
        }
        queue.0.push_back(RttJob {
            target: entity,
            text: sign.0.clone(),
            width: 512,
            height: 128,
            font_size: (86.0 - sign.0.len() as f32 * 2.4).clamp(30.0, 72.0),
            fg: Color::srgb(0.92, 0.96, 1.0),
            bg: Color::srgb(0.075, 0.09, 0.14),
            scroll: false,
            double_sided: true,
        });
        commands.entity(entity).insert(PendingScreen);
    }

    // Text marquees: nearest first, bounded count.
    if budget.text < TEXT_SCREEN_BUDGET {
        let mut candidates: Vec<_> = text_screens
            .iter()
            .map(|(e, t, s)| (t.translation().distance(player_pos), e, s))
            .filter(|(d, ..)| *d < SCREEN_ACTIVATE_DIST)
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, entity, screen) in candidates
            .into_iter()
            .take(TEXT_SCREEN_BUDGET - budget.text)
        {
            let mut text = read_prefix(&screen.path, 1600);
            if text.trim().is_empty() {
                text = "(empty or unreadable)".into();
            }
            let fg = if screen.kind == FileKind::Code {
                Color::srgb(0.62, 0.95, 0.72)
            } else {
                Color::srgb(0.92, 0.94, 0.98)
            };
            queue.0.push_back(RttJob {
                target: entity,
                text,
                width: 256,
                height: 1024,
                font_size: 17.0,
                fg,
                bg: Color::srgb(0.015, 0.03, 0.05),
                scroll: true,
                double_sided: false,
            });
            commands.entity(entity).insert(PendingScreen);
            budget.text += 1;
        }
    }

    // Image displays: decode on the async pool.
    if budget.images < IMAGE_SCREEN_BUDGET {
        let mut candidates: Vec<_> = image_screens
            .iter()
            .map(|(e, t, s)| (t.translation().distance(player_pos), e, s))
            .filter(|(d, ..)| *d < SCREEN_ACTIVATE_DIST)
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, entity, screen) in candidates
            .into_iter()
            .take(IMAGE_SCREEN_BUDGET - budget.images)
        {
            let path = screen.path.clone();
            let task = AsyncComputeTaskPool::get()
                .spawn(async move { decode_image(&path) });
            tasks.0.push((Some(entity), screen.path.clone(), task));
            commands.entity(entity).insert(PendingScreen);
            budget.images += 1;
        }
    }
}

/// Runs at most one offscreen text render at a time; a 2D camera draws the
/// text into a texture for a few frames, then the camera is despawned and the
/// texture is applied to the requesting panel as an emissive material.
fn process_rtt_queue(
    mut commands: Commands,
    mut queue: ResMut<RttQueue>,
    mut active: ResMut<RttActive>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    panels: Query<(), Or<(With<TextScreen>, With<SignText>)>>,
) {
    // Advance the in-flight job.
    if let Some(rtt) = active.0.as_mut() {
        rtt.frames_left = rtt.frames_left.saturating_sub(1);
        if rtt.frames_left > 0 {
            return;
        }
        let rtt = active.0.take().unwrap();
        commands.entity(rtt.camera).despawn();
        commands.entity(rtt.text_entity).despawn();

        // The panel might have been despawned meanwhile.
        if panels.get(rtt.job.target).is_ok() {
            let emissive = if rtt.job.scroll { 1.7 } else { 1.25 };
            let material = materials.add(StandardMaterial {
                base_color: Color::srgb(0.02, 0.02, 0.03),
                emissive: LinearRgba::WHITE * emissive,
                emissive_texture: Some(rtt.image.clone()),
                perceptual_roughness: 0.35,
                cull_mode: if rtt.job.double_sided {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                double_sided: rtt.job.double_sided,
                ..default()
            });
            let mut e = commands.entity(rtt.job.target);
            e.insert((
                MeshMaterial3d(material.clone()),
                ScreenReady {
                    image: rtt.image,
                    material,
                    is_text: rtt.job.scroll,
                },
            ));
            e.remove::<PendingScreen>();
            if rtt.job.scroll {
                let phase = (rtt.job.target.index().index() % 97) as f32 / 97.0;
                e.insert(ScrollText(phase));
            }
        } else {
            images.remove(&rtt.image);
        }
    }

    // Start the next job.
    let Some(job) = queue.0.pop_front() else { return };
    let size = Extent3d {
        width: job.width,
        height: job.height,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Bgra8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::RENDER_ATTACHMENT;
    // Repeat so scrolled UVs wrap around instead of smearing.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    let image_handle = images.add(image);

    let camera = commands
        .spawn((
            Camera2d,
            Camera {
                order: -10,
                clear_color: ClearColorConfig::Custom(job.bg),
                ..default()
            },
            RenderTarget::Image(image_handle.clone().into()),
            RenderLayers::layer(RTT_LAYER),
        ))
        .id();
    // Marquees hang top-anchored so long content scrolls up through the
    // panel; signs are centered on both axes.
    let (anchor, position, justify) = if job.scroll {
        (
            Anchor(Vec2::new(0.0, 0.5)),
            Vec3::new(0.0, job.height as f32 * 0.5 - 8.0, 0.0),
            Justify::Left,
        )
    } else {
        (Anchor(Vec2::ZERO), Vec3::ZERO, Justify::Center)
    };
    let text_entity = commands
        .spawn((
            Text2d::new(job.text.clone()),
            TextFont::from_font_size(job.font_size),
            TextColor(job.fg),
            TextLayout {
                justify,
                ..default()
            },
            TextBounds {
                width: Some(job.width as f32 - 18.0),
                height: None,
            },
            anchor,
            Transform::from_translation(position),
            RenderLayers::layer(RTT_LAYER),
        ))
        .id();

    active.0 = Some(ActiveRtt {
        job,
        camera,
        text_entity,
        image: image_handle,
        frames_left: 4,
    });
}

fn scroll_text_screens(
    screens: Query<(&MeshMaterial3d<StandardMaterial>, &ScrollText)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
) {
    for (material, scroll) in &screens {
        if let Some(mut mat) = materials.get_mut(&material.0) {
            let y = (time.elapsed_secs() * 0.05 + scroll.0).fract();
            mat.uv_transform.translation.y = y;
        }
    }
}

/// Turns off screens far from the player and frees their GPU memory.
fn deactivate_far_screens(
    mut commands: Commands,
    player: Query<&Transform, With<Player>>,
    screens: Query<(Entity, &GlobalTransform, &ScreenReady), Without<SignText>>,
    palette: Option<Res<Palette>>,
    inspector: Res<Inspector>,
    image_screens: Query<&ImageScreen>,
    mut budget: ResMut<ScreenBudget>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<ImageCache>,
) {
    let Some(palette) = palette else { return };
    let Ok(player_pos) = player.single().map(|t| t.translation) else {
        return;
    };
    // Never free the image currently shown in the inspector.
    let inspected_path = match inspector.0.as_ref() {
        Some(InspectorContent::Image { path, .. }) => Some(path.clone()),
        _ => None,
    };
    for (entity, transform, ready) in &screens {
        if transform.translation().distance(player_pos) < SCREEN_DEACTIVATE_DIST {
            continue;
        }
        if let Ok(screen) = image_screens.get(entity) {
            if Some(&screen.path) == inspected_path.as_ref() {
                continue;
            }
            cache.0.remove(&screen.path);
        }
        commands
            .entity(entity)
            .insert(MeshMaterial3d(palette.screen_off.clone()))
            .remove::<(ScreenReady, ScrollText, PendingScreen)>();
        images.remove(&ready.image);
        materials.remove(&ready.material);
        if ready.is_text {
            budget.text = budget.text.saturating_sub(1);
        } else {
            budget.images = budget.images.saturating_sub(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Image decoding
// ---------------------------------------------------------------------------

struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Resource, Default)]
struct ImageTasks(Vec<(Option<Entity>, PathBuf, Task<Option<DecodedImage>>)>);

fn decode_image(path: &Path) -> Option<DecodedImage> {
    let img = image::open(path).ok()?;
    let img = img.thumbnail(1024, 1024).to_rgba8();
    Some(DecodedImage {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    })
}

/// Inspector asked for an image with no active screen nearby.
fn handle_image_requests(
    mut requests: MessageReader<RequestImage>,
    cache: Res<ImageCache>,
    mut tasks: ResMut<ImageTasks>,
) {
    for RequestImage(path) in requests.read() {
        if cache.0.contains_key(path) || tasks.0.iter().any(|(_, p, _)| p == path) {
            continue;
        }
        let path_clone = path.clone();
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { decode_image(&path_clone) });
        tasks.0.push((None, path.clone(), task));
    }
}

fn poll_image_tasks(
    mut commands: Commands,
    mut tasks: ResMut<ImageTasks>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: ResMut<ImageCache>,
    mut budget: ResMut<ScreenBudget>,
    mut panels: Query<(&ImageScreen, &mut Transform, Has<Fitted>)>,
) {
    let mut finished = Vec::new();
    for (i, (_, _, task)) in tasks.0.iter_mut().enumerate() {
        if let Some(result) = block_on(poll_once(task)) {
            finished.push((i, result));
        }
    }
    // Remove back-to-front so indices stay valid.
    for (i, result) in finished.into_iter().rev() {
        let (panel, path, _) = tasks.0.remove(i);
        let Some(decoded) = result else {
            if let Some(panel_entity) = panel {
                budget.images = budget.images.saturating_sub(1);
                if let Ok(mut e) = commands.get_entity(panel_entity) {
                    e.insert(ScreenFailed);
                }
            }
            continue;
        };
        let image_handle = images.add(Image::new(
            Extent3d {
                width: decoded.width,
                height: decoded.height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            decoded.rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        cache.0.insert(path, image_handle.clone());

        let Some(panel_entity) = panel else { continue };
        let Ok((screen, mut transform, fitted)) = panels.get_mut(panel_entity) else {
            budget.images = budget.images.saturating_sub(1);
            continue;
        };
        // Fit the panel to the image aspect ratio (once).
        if !fitted {
            let img_aspect = decoded.width as f32 / decoded.height.max(1) as f32;
            let panel_aspect = screen.base_size.x / screen.base_size.y.max(0.01);
            if img_aspect > panel_aspect {
                transform.scale.y *= panel_aspect / img_aspect;
            } else {
                transform.scale.x *= img_aspect / panel_aspect;
            }
            commands.entity(panel_entity).insert(Fitted);
        }
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(image_handle.clone()),
            unlit: true,
            ..default()
        });
        commands
            .entity(panel_entity)
            .insert((
                MeshMaterial3d(material.clone()),
                ScreenReady {
                    image: image_handle,
                    material,
                    is_text: false,
                },
            ))
            .remove::<PendingScreen>();
    }
}

// ---------------------------------------------------------------------------
// Audio playback
// ---------------------------------------------------------------------------

/// Currently playing audio file, if any (path, display name, sink entity).
#[derive(Resource, Default)]
pub struct CurrentAudio(pub Option<(PathBuf, String, Entity)>);

#[derive(Resource, Default)]
struct AudioTask(Option<(PathBuf, String, Task<Option<Vec<u8>>>)>);

fn handle_play_audio(
    mut commands: Commands,
    mut messages: MessageReader<PlayAudio>,
    mut current: ResMut<CurrentAudio>,
    mut pending: ResMut<AudioTask>,
) {
    for msg in messages.read() {
        // Toggle off if the same file is already playing.
        if let Some((path, _, entity)) = current.0.as_ref() {
            let same = *path == msg.path;
            commands.entity(*entity).despawn();
            current.0 = None;
            if same {
                pending.0 = None;
                continue;
            }
        }
        let path = msg.path.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let meta = std::fs::metadata(&path).ok()?;
            if meta.len() > MAX_AUDIO_BYTES {
                return None;
            }
            std::fs::read(&path).ok()
        });
        pending.0 = Some((msg.path.clone(), msg.name.clone(), task));
    }
}

fn poll_audio_task(
    mut commands: Commands,
    mut pending: ResMut<AudioTask>,
    mut sources: ResMut<Assets<AudioSource>>,
    mut current: ResMut<CurrentAudio>,
) {
    let Some((_, _, task)) = pending.0.as_mut() else {
        return;
    };
    let Some(result) = block_on(poll_once(task)) else {
        return;
    };
    let (path, name, _) = pending.0.take().unwrap();
    let Some(bytes) = result else {
        warn!("could not read audio file {}", path.display());
        return;
    };
    let handle = sources.add(AudioSource {
        bytes: Arc::from(bytes.into_boxed_slice()),
    });
    let entity = commands
        .spawn((AudioPlayer(handle), PlaybackSettings::DESPAWN))
        .id();
    current.0 = Some((path, name, entity));
}

// ---------------------------------------------------------------------------
// Idle animations
// ---------------------------------------------------------------------------

fn animate_bobbers(mut query: Query<(&mut Transform, &Bobber)>, time: Res<Time>) {
    let t = time.elapsed_secs();
    for (mut transform, bobber) in &mut query {
        transform.translation.y = bobber.base_y + (t * 1.5 + bobber.phase).sin() * bobber.amp;
    }
}

fn pulse_orbs(
    palette: Option<Res<Palette>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
) {
    let Some(palette) = palette else { return };
    if let Some(mut mat) = materials.get_mut(&palette.orb) {
        let pulse = 1.15 + 0.55 * (time.elapsed_secs() * 2.3).sin();
        mat.emissive = LinearRgba::rgb(1.8, 0.6, 2.4) * pulse;
    }
}
