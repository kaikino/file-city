mod atmo;
mod buildings;
mod citygen;
mod filereps;
mod interact;
mod player;
mod scan;
mod ui;
mod viewers;

use avian3d::prelude::*;
use bevy::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use scan::{DirNode, ScanConfig, ScanTask};

#[derive(States, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Scanning,
    Playing,
}

/// Update-schedule ordering: player input/motion first, then interaction.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameSet {
    Player,
    Interact,
}

/// The scanned directory tree, available once scanning completes.
#[derive(Resource)]
pub struct CityTree(pub DirNode);

fn parse_args() -> ScanConfig {
    let mut cfg = ScanConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--depth" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.max_depth = v;
                }
            }
            "--max-files" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.max_files = v;
                }
            }
            "--shot" => {
                cfg.shot = args.next().map(PathBuf::from);
            }
            "--shot-view" => {
                if let Some(v) = args.next() {
                    cfg.shot_view = v;
                }
            }
            "--tod" => {
                cfg.tod = args.next().and_then(|v| v.parse().ok());
            }
            other if !other.starts_with('-') => {
                let expanded = if other == "~" || other.starts_with("~/") {
                    let home = std::env::var("HOME").unwrap_or_default();
                    PathBuf::from(other.replacen('~', &home, 1))
                } else {
                    PathBuf::from(other)
                };
                cfg.root = expanded;
            }
            other => {
                warn!("ignoring unknown argument: {other}");
            }
        }
    }
    cfg
}

fn main() {
    let cfg = parse_args();
    info!("scanning root: {}", cfg.root.display());

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "File City".into(),
                resolution: (1440u32, 900u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(PhysicsPlugins::default())
        .add_plugins((
            citygen::CityGenPlugin,
            atmo::AtmoPlugin,
            player::PlayerPlugin,
            interact::InteractPlugin,
            filereps::FileRepsPlugin,
            ui::UiPlugin,
        ))
        .insert_resource(cfg)
        .init_state::<AppState>()
        .configure_sets(Update, (GameSet::Player, GameSet::Interact).chain())
        .add_systems(Startup, setup_scan)
        .add_systems(
            Update,
            (poll_scan, animate_loading_screen).run_if(in_state(AppState::Scanning)),
        )
        .add_systems(OnExit(AppState::Scanning), teardown_loading_screen)
        .add_systems(Update, debug_screenshot.run_if(in_state(AppState::Playing)))
        .run();
}

/// With `--shot out.png`: capture a screenshot a few seconds into gameplay
/// and exit. Renders a dedicated camera into an offscreen texture, because
/// reading back the window surface is not supported everywhere.
/// `--shot-view street|neon|gallery|aerial|alley` picks the framing.
fn debug_screenshot(
    mut commands: Commands,
    cfg: Res<ScanConfig>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
    mut images: ResMut<Assets<Image>>,
    mut player_body: Query<(&mut Transform, &mut LinearVelocity), With<player::Player>>,
    signs: Query<&GlobalTransform, With<citygen::SignText>>,
    neon: Query<&GlobalTransform, With<citygen::NeonNameSign>>,
    galleries: Query<&GlobalTransform, With<citygen::ImageScreen>>,
    marquees: Query<&GlobalTransform, With<citygen::TextScreen>>,
    gates: Option<Res<citygen::Gates>>,
    meta: Option<Res<citygen::CityMeta>>,
    night: Option<Res<atmo::NightFactor>>,
    shot_target: Option<Res<ShotTarget>>,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::camera::{Hdr, RenderTarget};
    use bevy::pbr::DistanceFog;
    use bevy::post_process::bloom::Bloom;
    use bevy::render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureUsages,
    };
    use bevy::render::view::window::screenshot::{save_to_disk, Screenshot};

    let Some(path) = cfg.shot.clone() else { return };
    *frames += 1;

    let pose = shot_pose(
        &cfg.shot_view,
        &signs,
        &neon,
        &galleries,
        &marquees,
        gates.as_deref(),
        meta.as_deref(),
    );

    // Stand the player at the camera so nearby screens and neon light up.
    if *frames == 90 {
        if let Ok((mut transform, mut velocity)) = player_body.single_mut() {
            transform.translation = pose.eye;
            velocity.0 = Vec3::ZERO;
        }
    }

    if *frames == 360 {
        let mut image = Image::new_fill(
            Extent3d {
                width: 1920,
                height: 1080,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Bgra8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::COPY_SRC
            | TextureUsages::RENDER_ATTACHMENT;
        let handle = images.add(image);
        let nf = night.map(|n| n.0).unwrap_or(0.7);
        let fog_end = meta
            .map(|m| (m.half_extent * 2.4).clamp(150.0, 420.0) * (1.0 - 0.42 * nf))
            .unwrap_or(220.0);
        let fog_color = Vec3::new(0.62, 0.74, 0.88).lerp(Vec3::new(0.05, 0.045, 0.11), nf);
        let transform = Transform::from_translation(pose.eye).looking_at(pose.at, Vec3::Y);
        commands.spawn((
            Camera3d::default(),
            Hdr,
            bevy::pbr::AtmosphereSettings::default(),
            bevy::light::AtmosphereEnvironmentMapLight::default(),
            Camera {
                order: 5,
                ..default()
            },
            RenderTarget::Image(handle.clone().into()),
            Projection::Perspective(PerspectiveProjection {
                fov: pose.fov.to_radians(),
                ..default()
            }),
            transform,
            Bloom::NATURAL,
            bevy::camera::Exposure {
                ev100: 9.7 - 1.8 * nf,
            },
            DistanceFog {
                color: Color::srgb(fog_color.x, fog_color.y, fog_color.z),
                falloff: FogFalloff::Linear {
                    start: fog_end * (0.35 - 0.16 * nf),
                    end: fog_end,
                },
                ..default()
            },
        ));
        commands.insert_resource(ShotTarget(handle));
    }
    if *frames == 480 {
        if let Some(target) = shot_target {
            commands
                .spawn(Screenshot(RenderTarget::Image(target.0.clone().into())))
                .observe(save_to_disk(path));
        }
    }
    if *frames > 560 {
        exit.write(AppExit::Success);
    }
}

struct ShotPose {
    eye: Vec3,
    at: Vec3,
    fov: f32,
}

fn shot_pose(
    view: &str,
    signs: &Query<&GlobalTransform, With<citygen::SignText>>,
    neon: &Query<&GlobalTransform, With<citygen::NeonNameSign>>,
    galleries: &Query<&GlobalTransform, With<citygen::ImageScreen>>,
    marquees: &Query<&GlobalTransform, With<citygen::TextScreen>>,
    gates: Option<&citygen::Gates>,
    meta: Option<&citygen::CityMeta>,
) -> ShotPose {
    let origin = Vec3::new(0.0, 2.0, 0.0);
    match view {
        "aerial" => {
            let half = meta.map(|m| m.half_extent).unwrap_or(40.0);
            ShotPose {
                eye: Vec3::new(-half * 0.55, 22.0, half * 0.85),
                at: Vec3::new(0.0, 3.5, -half * 0.05),
                fov: 52.0,
            }
        }
        "neon" => {
            let g = neon
                .iter()
                .map(|t| t)
                .min_by(|a, b| {
                    a.translation()
                        .distance_squared(origin)
                        .total_cmp(&b.translation().distance_squared(origin))
                });
            if let Some(g) = g {
                // Quads face +Z (Bevy back()); forward() would put us behind the sign.
                let face = g.back();
                ShotPose {
                    eye: g.translation() + face * 6.0 + Vec3::Y * 0.15,
                    at: g.translation() + Vec3::Y * -0.2,
                    fov: 52.0,
                }
            } else {
                default_street(meta, signs, gates)
            }
        }
        "gallery" => {
            let g = galleries.iter().min_by(|a, b| {
                a.translation()
                    .distance_squared(origin)
                    .total_cmp(&b.translation().distance_squared(origin))
            });
            if let Some(g) = g {
                let face = g.back();
                ShotPose {
                    eye: g.translation() + face * 8.0 + Vec3::Y * 0.1,
                    at: g.translation() + Vec3::Y * -0.3,
                    fov: 55.0,
                }
            } else {
                default_street(meta, signs, gates)
            }
        }
        "alley" => {
            if let Some((gate, out)) = gates.and_then(|g| g.0.first()).copied() {
                let out3 = Vec3::new(out.x, 0.0, out.y);
                ShotPose {
                    eye: gate + Vec3::Y * 1.65 + out3 * 5.5,
                    at: gate + Vec3::Y * 2.6 - out3 * 16.0,
                    fov: 66.0,
                }
            } else {
                default_street(meta, signs, gates)
            }
        }
        "marquee" => {
            let g = marquees.iter().min_by(|a, b| {
                a.translation()
                    .distance_squared(origin)
                    .total_cmp(&b.translation().distance_squared(origin))
            });
            if let Some(g) = g {
                let face = g.back();
                ShotPose {
                    eye: g.translation() + face * 7.2 + Vec3::new(1.4, 0.0, 0.0),
                    at: g.translation() + Vec3::Y * -0.4,
                    fov: 58.0,
                }
            } else {
                default_street(meta, signs, gates)
            }
        }
        _ => default_street(meta, signs, gates),
    }
}

fn default_street(
    meta: Option<&citygen::CityMeta>,
    signs: &Query<&GlobalTransform, With<citygen::SignText>>,
    gates: Option<&citygen::Gates>,
) -> ShotPose {
    let _ = (signs, gates);
    if let Some(m) = meta {
        return ShotPose {
            eye: m.spawn_pos + Vec3::new(0.0, 0.4, 2.0),
            at: Vec3::new(0.0, 4.0, m.spawn_pos.z - 30.0),
            fov: 62.0,
        };
    }
    let sign = signs
        .iter()
        .next()
        .map(|g| g.translation())
        .unwrap_or(Vec3::new(0.0, 4.0, 0.0));
    ShotPose {
        eye: sign + Vec3::new(14.0, -1.4, 2.0),
        at: sign + Vec3::new(-24.0, -2.2, 0.0),
        fov: 64.0,
    }
}

#[derive(Resource)]
struct ShotTarget(Handle<Image>);

#[derive(Component)]
struct LoadingUi;

#[derive(Component)]
struct LoadingProgressText;

#[derive(Component)]
struct LoadingCamera;

fn setup_scan(mut commands: Commands, cfg: Res<ScanConfig>) {
    commands.insert_resource(scan::start_scan(&cfg));

    // Temporary camera so the loading UI has something to render to.
    commands.spawn((Camera3d::default(), LoadingCamera));

    commands
        .spawn((
            LoadingUi,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(18.0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.05, 0.06, 0.10)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("FILE CITY"),
                TextFont::from_font_size(64.0),
                TextColor(Color::srgb(0.85, 0.92, 1.0)),
            ));
            parent.spawn((
                Text::new(format!("Scanning {}", cfg.root.display())),
                TextFont::from_font_size(20.0),
                TextColor(Color::srgb(0.55, 0.62, 0.75)),
            ));
            parent.spawn((
                LoadingProgressText,
                Text::new("0 files found"),
                TextFont::from_font_size(24.0),
                TextColor(Color::srgb(0.95, 0.75, 0.35)),
            ));
        });
}

fn animate_loading_screen(
    task: Res<ScanTask>,
    time: Res<Time>,
    mut query: Query<&mut Text, With<LoadingProgressText>>,
) {
    let n = task.progress.load(Ordering::Relaxed);
    let dots = ".".repeat(1 + (time.elapsed_secs() * 2.0) as usize % 3);
    for mut text in &mut query {
        text.0 = format!("{n} files found{dots}");
    }
}

fn poll_scan(
    mut commands: Commands,
    task: Res<ScanTask>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let received = task.receiver.lock().ok().and_then(|rx| rx.try_recv().ok());
    if let Some(tree) = received {
        info!(
            "scan complete: {} files under {}",
            tree.file_count(),
            tree.path.display()
        );
        commands.insert_resource(CityTree(tree));
        commands.remove_resource::<ScanTask>();
        next_state.set(AppState::Playing);
    }
}

fn teardown_loading_screen(
    mut commands: Commands,
    ui: Query<Entity, With<LoadingUi>>,
    cam: Query<Entity, With<LoadingCamera>>,
) {
    for e in ui.iter().chain(cam.iter()) {
        commands.entity(e).despawn();
    }
}
