//! HUD (crosshair, tooltip, breadcrumb, hints, now-playing) and the
//! fullscreen inspector overlay for reading/viewing files.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::ScrollPosition;

use crate::citygen::Districts;
use crate::filereps::{CurrentAudio, ImageCache, RequestImage};
use crate::interact::{Hovered, Inspector, InspectorContent};
use crate::player::{CursorGrabbed, Player};
use crate::scan::human_size;
use crate::AppState;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Playing), build_hud).add_systems(
            Update,
            (
                update_tooltip,
                update_breadcrumb,
                update_capture_prompt,
                update_now_playing,
                rebuild_inspector,
                fill_inspector_image,
                scroll_inspector,
            )
                .run_if(in_state(AppState::Playing)),
        );
    }
}

const PANEL_BG: Color = Color::srgba(0.07, 0.085, 0.12, 0.97);
const TEXT_DIM: Color = Color::srgb(0.55, 0.62, 0.72);
const TEXT_MAIN: Color = Color::srgb(0.92, 0.94, 0.97);
const ACCENT: Color = Color::srgb(1.0, 0.75, 0.35);

#[derive(Component)]
struct TooltipRoot;
#[derive(Component)]
struct TooltipName;
#[derive(Component)]
struct TooltipMeta;
#[derive(Component)]
struct TooltipHint;
#[derive(Component)]
struct Breadcrumb;
#[derive(Component)]
struct CapturePrompt;
#[derive(Component)]
struct NowPlaying;
#[derive(Component)]
struct InspectorRoot;
#[derive(Component)]
struct InspectorScroll;
#[derive(Component)]
struct InspectorImageSlot(std::path::PathBuf);

fn build_hud(mut commands: Commands) {
    // Crosshair.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Percent(50.0),
            width: Val::Px(6.0),
            height: Val::Px(6.0),
            margin: UiRect {
                left: Val::Px(-3.0),
                top: Val::Px(-3.0),
                ..default()
            },
            border_radius: BorderRadius::MAX,
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
    ));

    // Hovered-file tooltip above the bottom edge.
    commands
        .spawn((
            TooltipRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                bottom: Val::Px(64.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(2.0),
                padding: UiRect::axes(Val::Px(18.0), Val::Px(10.0)),
                margin: UiRect::left(Val::Px(-220.0)),
                width: Val::Px(440.0),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.09, 0.82)),
            Visibility::Hidden,
        ))
        .with_children(|parent| {
            parent.spawn((
                TooltipName,
                Text::new(""),
                TextFont::from_font_size(20.0),
                TextColor(TEXT_MAIN),
            ));
            parent.spawn((
                TooltipMeta,
                Text::new(""),
                TextFont::from_font_size(14.0),
                TextColor(TEXT_DIM),
            ));
            parent.spawn((
                TooltipHint,
                Text::new(""),
                TextFont::from_font_size(14.0),
                TextColor(ACCENT),
            ));
        });

    // Breadcrumb: which district (directory) you are standing in.
    commands.spawn((
        Breadcrumb,
        Text::new(""),
        TextFont::from_font_size(15.0),
        TextColor(TEXT_MAIN),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(14.0),
            top: Val::Px(12.0),
            ..default()
        },
    ));

    // Now playing (audio), top right.
    commands.spawn((
        NowPlaying,
        Text::new(""),
        TextFont::from_font_size(15.0),
        TextColor(ACCENT),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(14.0),
            top: Val::Px(12.0),
            ..default()
        },
    ));

    // Controls reference, bottom right.
    commands.spawn((
        Text::new("WASD move · Shift sprint · Space jump\nE inspect · F grab · Click shoot/throw"),
        TextFont::from_font_size(13.0),
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.45)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(14.0),
            bottom: Val::Px(12.0),
            ..default()
        },
    ));

    // Click-to-capture prompt.
    commands.spawn((
        CapturePrompt,
        Text::new("Click to capture the mouse"),
        TextFont::from_font_size(22.0),
        TextColor(TEXT_MAIN),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Percent(58.0),
            margin: UiRect::left(Val::Px(-140.0)),
            ..default()
        },
    ));
}

fn update_tooltip(
    hovered: Res<Hovered>,
    mut root: Query<&mut Visibility, With<TooltipRoot>>,
    mut name: Query<&mut Text, (With<TooltipName>, Without<TooltipMeta>, Without<TooltipHint>)>,
    mut meta: Query<&mut Text, (With<TooltipMeta>, Without<TooltipName>, Without<TooltipHint>)>,
    mut hint: Query<&mut Text, (With<TooltipHint>, Without<TooltipName>, Without<TooltipMeta>)>,
) {
    if !hovered.is_changed() {
        return;
    }
    let Ok(mut visibility) = root.single_mut() else {
        return;
    };
    match hovered.0.as_ref() {
        Some(info) => {
            *visibility = Visibility::Visible;
            if let Ok(mut t) = name.single_mut() {
                t.0 = info.file.name.clone();
            }
            if let Ok(mut t) = meta.single_mut() {
                t.0 = format!(
                    "{} · {}",
                    info.file.kind.label(),
                    human_size(info.file.size)
                );
            }
            if let Ok(mut t) = hint.single_mut() {
                let mut hints: Vec<&str> = Vec::new();
                let close_enough = info.distance <= 14.0;
                match info.file.kind {
                    crate::scan::FileKind::Audio if close_enough => {
                        hints.push("E play/stop");
                    }
                    crate::scan::FileKind::Archive if close_enough => {
                        hints.push("E list contents");
                    }
                    crate::scan::FileKind::Executable
                    | crate::scan::FileKind::Data
                    | crate::scan::FileKind::Other
                        if close_enough =>
                    {
                        hints.push("E hex view");
                    }
                    _ if close_enough => hints.push("E inspect"),
                    _ => {}
                }
                if info.is_prop && info.distance <= 5.0 {
                    hints.push("F grab");
                }
                t.0 = hints.join(" · ");
            }
        }
        None => *visibility = Visibility::Hidden,
    }
}

fn update_breadcrumb(
    districts: Option<Res<Districts>>,
    player: Query<&Transform, With<Player>>,
    mut text: Query<&mut Text, With<Breadcrumb>>,
    time: Res<Time>,
    mut last: Local<f32>,
) {
    if time.elapsed_secs() - *last < 0.25 {
        return;
    }
    *last = time.elapsed_secs();
    let (Some(districts), Ok(transform), Ok(mut text)) =
        (districts, player.single(), text.single_mut())
    else {
        return;
    };
    let p = Vec2::new(transform.translation.x, transform.translation.z);
    let label = districts
        .0
        .iter()
        .filter(|d| d.rect.contains(p))
        .max_by_key(|d| d.depth)
        .map(|d| d.display_path.clone())
        .unwrap_or_default();
    text.0 = label;
}

fn update_capture_prompt(
    grab: Res<CursorGrabbed>,
    inspector: Res<Inspector>,
    mut prompt: Query<&mut Visibility, With<CapturePrompt>>,
) {
    if let Ok(mut visibility) = prompt.single_mut() {
        *visibility = if !grab.0 && inspector.0.is_none() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn update_now_playing(current: Res<CurrentAudio>, mut text: Query<&mut Text, With<NowPlaying>>) {
    if !current.is_changed() {
        return;
    }
    if let Ok(mut t) = text.single_mut() {
        t.0 = match current.0.as_ref() {
            Some((_, name, _)) => format!("♪ {name}"),
            None => String::new(),
        };
    }
}

// ---------------------------------------------------------------------------
// Inspector overlay
// ---------------------------------------------------------------------------

fn rebuild_inspector(
    mut commands: Commands,
    inspector: Res<Inspector>,
    existing: Query<Entity, With<InspectorRoot>>,
    cache: Res<ImageCache>,
    mut request_image: MessageWriter<RequestImage>,
) {
    if !inspector.is_changed() {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    let Some(content) = inspector.0.as_ref() else {
        return;
    };

    let root = commands
        .spawn((
            InspectorRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.66)),
            GlobalZIndex(10),
        ))
        .id();

    let panel = commands
        .spawn((
            Node {
                width: Val::Percent(74.0),
                height: Val::Percent(82.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(22.0)),
                row_gap: Val::Px(10.0),
                border_radius: BorderRadius::all(Val::Px(14.0)),
                ..default()
            },
            BackgroundColor(PANEL_BG),
            ChildOf(root),
        ))
        .id();

    let (title, meta) = match content {
        InspectorContent::Text { title, meta, .. } => (title.clone(), meta.clone()),
        InspectorContent::Image { title, meta, .. } => (title.clone(), meta.clone()),
        InspectorContent::Info { title, .. } => (title.clone(), String::new()),
    };
    commands.spawn((
        Text::new(title),
        TextFont::from_font_size(24.0),
        TextColor(TEXT_MAIN),
        ChildOf(panel),
    ));
    if !meta.is_empty() {
        commands.spawn((
            Text::new(meta),
            TextFont::from_font_size(13.0),
            TextColor(TEXT_DIM),
            ChildOf(panel),
        ));
    }

    match content {
        InspectorContent::Text { body, .. } => {
            let scroll = commands
                .spawn((
                    InspectorScroll,
                    Node {
                        flex_grow: 1.0,
                        overflow: Overflow::scroll_y(),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(Val::Px(12.0)),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.02, 0.03, 0.05, 0.9)),
                    ScrollPosition(Vec2::ZERO),
                    ChildOf(panel),
                ))
                .id();
            commands.spawn((
                Text::new(body.clone()),
                TextFont::from_font_size(15.0),
                TextColor(Color::srgb(0.80, 0.88, 0.82)),
                ChildOf(scroll),
            ));
        }
        InspectorContent::Image { path, .. } => {
            let holder = commands
                .spawn((
                    Node {
                        flex_grow: 1.0,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    ChildOf(panel),
                ))
                .id();
            if let Some(handle) = cache.0.get(path) {
                commands.spawn((
                    ImageNode::new(handle.clone()),
                    Node {
                        max_width: Val::Percent(100.0),
                        max_height: Val::Percent(100.0),
                        ..default()
                    },
                    ChildOf(holder),
                ));
            } else {
                request_image.write(RequestImage(path.clone()));
                commands.spawn((
                    InspectorImageSlot(path.clone()),
                    Text::new("Loading image…"),
                    TextFont::from_font_size(18.0),
                    TextColor(TEXT_DIM),
                    ChildOf(holder),
                ));
            }
        }
        InspectorContent::Info { lines, .. } => {
            let body = commands
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(6.0),
                        padding: UiRect::all(Val::Px(12.0)),
                        ..default()
                    },
                    ChildOf(panel),
                ))
                .id();
            for line in lines {
                commands.spawn((
                    Text::new(line.clone()),
                    TextFont::from_font_size(16.0),
                    TextColor(TEXT_MAIN),
                    ChildOf(body),
                ));
            }
        }
    }

    commands.spawn((
        Text::new("E / Esc close · scroll to read"),
        TextFont::from_font_size(13.0),
        TextColor(TEXT_DIM),
        ChildOf(panel),
    ));
}

/// Swap the "Loading image…" placeholder for the picture once decoded.
fn fill_inspector_image(
    mut commands: Commands,
    slots: Query<(Entity, &InspectorImageSlot, &ChildOf)>,
    cache: Res<ImageCache>,
) {
    for (entity, slot, child_of) in &slots {
        if let Some(handle) = cache.0.get(&slot.0) {
            commands.entity(entity).despawn();
            commands.spawn((
                ImageNode::new(handle.clone()),
                Node {
                    max_width: Val::Percent(100.0),
                    max_height: Val::Percent(100.0),
                    ..default()
                },
                ChildOf(child_of.0),
            ));
        }
    }
}

fn scroll_inspector(
    mut wheel: MessageReader<MouseWheel>,
    mut scrollers: Query<&mut ScrollPosition, With<InspectorScroll>>,
) {
    for event in wheel.read() {
        let dy = match event.unit {
            bevy::input::mouse::MouseScrollUnit::Line => event.y * 36.0,
            bevy::input::mouse::MouseScrollUnit::Pixel => event.y,
        };
        for mut scroll in &mut scrollers {
            scroll.0.y = (scroll.0.y - dy).max(0.0);
        }
    }
}
