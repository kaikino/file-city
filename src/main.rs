mod citygen;
mod player;
mod scan;

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
        .add_plugins((citygen::CityGenPlugin, player::PlayerPlugin))
        .insert_resource(cfg)
        .init_state::<AppState>()
        .add_systems(Startup, (setup_scan, setup_lighting))
        .add_systems(
            Update,
            (poll_scan, animate_loading_screen).run_if(in_state(AppState::Scanning)),
        )
        .add_systems(OnExit(AppState::Scanning), teardown_loading_screen)
        .run();
}

fn setup_lighting(mut commands: Commands, mut ambient: ResMut<GlobalAmbientLight>) {
    ambient.color = Color::srgb(0.75, 0.85, 1.0);
    ambient.brightness = 220.0;
    commands.insert_resource(ClearColor(Color::srgb(0.48, 0.65, 0.86)));
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.96, 0.88),
            illuminance: 9200.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(
            EulerRot::YXZ,
            35f32.to_radians(),
            -52f32.to_radians(),
            0.0,
        )),
    ));
}

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
