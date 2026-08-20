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
/// and exit. Renders a mirror of the player camera into an offscreen texture,
/// because reading back the window surface is not supported everywhere.
fn debug_screenshot(
    mut commands: Commands,
    cfg: Res<ScanConfig>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
    mut images: ResMut<Assets<Image>>,
    player_cam: Query<&GlobalTransform, With<player::PlayerCamera>>,
    mut player_body: Query<(&mut Transform, &mut LinearVelocity), With<player::Player>>,
    signs: Query<&GlobalTransform, With<citygen::SignText>>,
    shot_target: Option<Res<ShotTarget>>,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::camera::{Hdr, RenderTarget};
    use bevy::post_process::bloom::Bloom;
    use bevy::render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureUsages,
    };
    use bevy::render::view::window::screenshot::{save_to_disk, Screenshot};

    let Some(path) = cfg.shot.clone() else { return };
    *frames += 1;

    // Drop the player into the city center so the capture shows active
    // screens, signs and props at close range.
    if *frames == 100 {
        if let Ok((mut transform, mut velocity)) = player_body.single_mut() {
            transform.translation = Vec3::new(0.0, 6.0, 0.0);
            velocity.0 = Vec3::ZERO;
        }
    }

    if *frames == 260 {
        let mut image = Image::new_fill(
            Extent3d {
                width: 1280,
                height: 800,
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
        // Frame the nearest district sign from a raised vantage so the shot
        // shows signs, screens and the city layout together.
        let player_pos = player_cam
            .single()
            .map(|g| g.translation())
            .unwrap_or(Vec3::new(0.0, 2.0, 0.0));
        let sign_pos = signs
            .iter()
            .map(|g| g.translation())
            .min_by(|a, b| {
                a.distance_squared(player_pos)
                    .total_cmp(&b.distance_squared(player_pos))
            })
            .unwrap_or(player_pos + Vec3::new(0.0, 0.0, -20.0));
        // Street-level view down the road the sign hangs over, so facades,
        // neon and screens line both sides of the frame.
        let transform = Transform::from_translation(sign_pos + Vec3::new(17.0, -1.6, 1.5))
            .looking_at(sign_pos + Vec3::new(-30.0, -2.4, 0.5), Vec3::Y);
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
                fov: 62f32.to_radians(),
                ..default()
            }),
            transform,
            Bloom::NATURAL,
            bevy::camera::Exposure { ev100: 9.7 },
        ));
        commands.insert_resource(ShotTarget(handle));
    }
    if *frames == 330 {
        if let Some(target) = shot_target {
            commands
                .spawn(Screenshot(RenderTarget::Image(target.0.clone().into())))
                .observe(save_to_disk(path));
        }
    }
    if *frames > 400 {
        exit.write(AppExit::Success);
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
