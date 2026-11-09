//! Create: a browser over everything a scene can gain, in place of a flat menu. Categories
//! down the left (Recent, All, then Basic, Shapes, Lights, …), a search that takes typing at
//! once, rows with an icon, a name and what the object is for, and a footer that says what
//! the highlighted object adds and where it goes. Arrows move, Enter creates, Escape closes.
//! Opened from the title bar, the Hierarchy's +, the palette or Shift+A; Shift+A over the
//! Scene view places the object on the ground under the pointer.
//!
//! Every entry is a registry `create.*` command (the same ids agents and menus use) whose
//! components the schema has; creation is one `entity.create` through `cmd.apply`.
use ara_kit::icons;
use ara_kit::ui::*;
use serde_json::{Value, json};
use stillkit::Rgba;
use stillkit::el::{Align, Axis, El, Handled, Justify, Overlay, col, el, icon, popover, row, text};
use stillkit::icons::Icon;

use super::Studio;
use crate::shell::commands::Target;

/// A browser category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// Empty entities and grouping.
    Basic,
    /// Primitive meshes.
    Shapes,
    /// Lights, sky and fog.
    Lights,
    /// Viewpoints.
    Camera,
    /// Post-process volumes.
    Post,
    /// Particle effects.
    Effects,
    /// Sound sources and listeners.
    Audio,
    /// Bodies, colliders, triggers and characters.
    Physics,
    /// Navigation surfaces and links.
    Navigation,
    /// Sprites and tile maps.
    TwoD,
    /// Ready-made gameplay objects.
    Gameplay,
}

impl Category {
    const ALL: [Category; 11] = [
        Category::Basic,
        Category::Shapes,
        Category::Lights,
        Category::Camera,
        Category::Post,
        Category::Effects,
        Category::Audio,
        Category::Physics,
        Category::Navigation,
        Category::TwoD,
        Category::Gameplay,
    ];

    fn title(self) -> &'static str {
        match self {
            Category::Basic => "Basic",
            Category::Shapes => "Shapes",
            Category::Lights => "Lights",
            Category::Camera => "Camera",
            Category::Post => "Post-processing",
            Category::Effects => "Effects",
            Category::Audio => "Audio",
            Category::Physics => "Physics",
            Category::Navigation => "Navigation",
            Category::TwoD => "2D",
            Category::Gameplay => "Gameplay",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Category::Basic => icons::LAYERS,
            Category::Shapes => icons::SHAPES,
            Category::Lights => icons::LIGHTBULB,
            Category::Camera => icons::VIDEO,
            Category::Post => icons::APERTURE,
            Category::Effects => icons::SPARKLES,
            Category::Audio => icons::VOLUME_2,
            Category::Physics => icons::ATOM,
            Category::Navigation => icons::ROUTE,
            Category::TwoD => icons::IMAGE,
            Category::Gameplay => icons::GAMEPAD_2,
        }
    }
}

/// How an entry makes its entity.
#[derive(Clone, Copy, Debug)]
enum Make {
    /// `entity.create` with these components (JSON), as a child of the target when set.
    Entity {
        components: &'static str,
        child: bool,
    },
    /// The selected roots under a new parent (the registry's `create.group`).
    Group,
}

/// One thing the browser creates.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    /// The registry command it is (`create.cube`).
    pub id: &'static str,
    /// Its name, also the new entity's.
    pub title: &'static str,
    /// Its category.
    pub category: Category,
    icon: Icon,
    /// What it is for, one line.
    pub blurb: &'static str,
    /// Extra words search matches.
    keywords: &'static str,
    make: Make,
}

const fn entity(
    id: &'static str,
    title: &'static str,
    category: Category,
    icon: Icon,
    blurb: &'static str,
    keywords: &'static str,
    components: &'static str,
) -> Item {
    Item {
        id,
        title,
        category,
        icon,
        blurb,
        keywords,
        make: Make::Entity {
            components,
            child: false,
        },
    }
}

/// Everything the browser offers, in browse order.
pub const CATALOG: &[Item] = &[
    entity(
        "create.empty",
        "Empty",
        Category::Basic,
        icons::SQUARE_DASHED,
        "An entity with only a Transform: a pivot, a folder, a spawn marker.",
        "entity node null pivot",
        "{}",
    ),
    Item {
        id: "create.empty_child",
        title: "Empty Child",
        category: Category::Basic,
        icon: icons::FOLDER_TREE,
        blurb: "An empty entity under the selection.",
        keywords: "child parent node",
        make: Make::Entity {
            components: "{}",
            child: true,
        },
    },
    Item {
        id: "create.group",
        title: "Group Selection",
        category: Category::Basic,
        icon: icons::BOXES,
        blurb: "A new parent for the selected roots, keeping where they are.",
        keywords: "parent folder",
        make: Make::Group,
    },
    entity(
        "create.cube",
        "Cube",
        Category::Shapes,
        icons::BOX,
        "A 1 m box mesh.",
        "box block mesh primitive",
        r#"{"MeshRenderer":{"shape":"box"}}"#,
    ),
    entity(
        "create.sphere",
        "Sphere",
        Category::Shapes,
        icons::CIRCLE,
        "A sphere mesh, 1 m across.",
        "ball mesh primitive",
        r#"{"MeshRenderer":{"shape":"sphere"}}"#,
    ),
    entity(
        "create.plane",
        "Plane",
        Category::Shapes,
        icons::SQUARE,
        "An upward-facing 1 m quad: floors and walls.",
        "quad floor ground mesh primitive",
        r#"{"MeshRenderer":{"shape":"plane"}}"#,
    ),
    entity(
        "create.capsule",
        "Capsule",
        Category::Shapes,
        icons::PILL,
        "An upright capsule: a stand-in character.",
        "pill mesh primitive",
        r#"{"MeshRenderer":{"shape":"capsule"}}"#,
    ),
    entity(
        "create.light.sun",
        "Sun",
        Category::Lights,
        icons::SUN,
        "A directional light with cascaded shadows: daylight.",
        "directional light sunlight",
        r#"{"Light":{"kind":"directional","intensity":3},"Transform":{"position":[0,6,0],"rotation":[-50,30,0]}}"#,
    ),
    entity(
        "create.light.point",
        "Point Light",
        Category::Lights,
        icons::LIGHTBULB,
        "Light in every direction from a point: lamps, fires.",
        "omni bulb lamp",
        r#"{"Light":{"kind":"point"}}"#,
    ),
    entity(
        "create.light.spot",
        "Spot Light",
        Category::Lights,
        icons::FLASHLIGHT,
        "A cone of light along −Z: torches, stage lights.",
        "cone torch flashlight",
        r#"{"Light":{"kind":"spot"}}"#,
    ),
    entity(
        "create.light.sky",
        "Sky & Atmosphere",
        Category::Lights,
        icons::CLOUD,
        "A procedural sky with exposure: the scene's ambient light.",
        "environment skybox atmosphere",
        r#"{"Environment":{}}"#,
    ),
    entity(
        "create.fog",
        "Height Fog",
        Category::Lights,
        icons::CLOUD,
        "Fog that thickens with depth and lower altitude.",
        "mist haze atmosphere",
        r#"{"HeightFog":{}}"#,
    ),
    entity(
        "create.light.probe",
        "Reflection Probe",
        Category::Lights,
        icons::GLOBE,
        "Captures surroundings for local reflections.",
        "cubemap reflections",
        r#"{"ReflectionProbe":{}}"#,
    ),
    entity(
        "create.camera",
        "Camera",
        Category::Camera,
        icons::VIDEO,
        "A viewpoint looking along −Z: what the player sees.",
        "view eye",
        r#"{"Camera":{}}"#,
    ),
    entity(
        "create.post.global",
        "Global Volume",
        Category::Post,
        icons::APERTURE,
        "Post-processing for the whole scene: exposure, bloom, grading.",
        "post process global bloom exposure tonemap",
        r#"{"PostProcessVolume":{"shape":"global"}}"#,
    ),
    entity(
        "create.post.box",
        "Box Volume",
        Category::Post,
        icons::BOX,
        "Post-processing that blends in near the camera inside a box.",
        "post process local area",
        r#"{"PostProcessVolume":{"shape":"box","blendDistance":2,"priority":10},"Transform":{"scale":[10,5,10]}}"#,
    ),
    entity(
        "create.post.sphere",
        "Sphere Volume",
        Category::Post,
        icons::CIRCLE,
        "Post-processing that blends in near the camera inside a sphere.",
        "post process local area",
        r#"{"PostProcessVolume":{"shape":"sphere","blendDistance":2,"priority":10},"Transform":{"scale":[8,8,8]}}"#,
    ),
    entity(
        "create.vfx",
        "VFX System",
        Category::Effects,
        icons::SPARKLES,
        "Plays a VFX graph: sparks, smoke, magic.",
        "particles particle system emitter",
        r#"{"VisualEffect":{}}"#,
    ),
    entity(
        "create.audio.source",
        "Audio Source",
        Category::Audio,
        icons::VOLUME_2,
        "A positional sound emitter.",
        "sound speaker music sfx",
        r#"{"AudioSource":{}}"#,
    ),
    entity(
        "create.audio.listener",
        "Listener",
        Category::Audio,
        icons::EAR,
        "Where the mix is heard from, usually on the camera.",
        "ear microphone",
        r#"{"AudioListener":{}}"#,
    ),
    entity(
        "create.physics.body",
        "Rigid Body",
        Category::Physics,
        icons::BOX,
        "A box that falls, collides and can be pushed.",
        "dynamic rigidbody physics crate",
        r#"{"MeshRenderer":{},"RigidBody":{},"Collider":{}}"#,
    ),
    entity(
        "create.physics.collider",
        "Static Collider",
        Category::Physics,
        icons::SQUARE,
        "A shape things collide with that never moves.",
        "static wall blocker",
        r#"{"Collider":{}}"#,
    ),
    entity(
        "create.physics.trigger",
        "Trigger Volume",
        Category::Physics,
        icons::SCAN,
        "A sensor volume that reports what enters it.",
        "sensor zone area overlap",
        r#"{"Collider":{"sensor":true}}"#,
    ),
    entity(
        "create.physics.character",
        "Character Controller",
        Category::Physics,
        icons::PERSON_STANDING,
        "A kinematic capsule that walks, climbs steps and slides.",
        "player kinematic capsule",
        r#"{"CharacterController":{},"MeshRenderer":{"shape":"capsule","size":[0.6,1.8,0.6]}}"#,
    ),
    entity(
        "create.nav.volume",
        "Nav Mesh Surface",
        Category::Navigation,
        icons::MAP,
        "Bakes where agents can walk.",
        "navmesh pathfinding ai",
        r#"{"NavMeshSurface":{}}"#,
    ),
    entity(
        "create.nav.link",
        "Nav Link",
        Category::Navigation,
        icons::LINK,
        "An off-mesh jump or ladder between two points.",
        "offmesh jump ladder",
        r#"{"NavLink":{}}"#,
    ),
    entity(
        "create.2d.sprite",
        "Sprite",
        Category::TwoD,
        icons::IMAGE,
        "A 2D image in the world.",
        "2d image texture",
        r#"{"Sprite":{}}"#,
    ),
    entity(
        "create.2d.tilemap",
        "Tilemap",
        Category::TwoD,
        icons::GRID_2X2,
        "A grid of tiles for 2D levels.",
        "2d tiles grid level",
        r#"{"Tilemap":{}}"#,
    ),
    entity(
        "create.gameplay.player",
        "Player",
        Category::Gameplay,
        icons::GAMEPAD_2,
        "A capsule you can drive with the keyboard or a pad.",
        "character controller pawn",
        r#"{"PlayerControl":{},"MeshRenderer":{"shape":"capsule","size":[0.6,1.8,0.6]}}"#,
    ),
];

/// The catalog entry for registry command `id`.
pub fn item(id: &str) -> Option<&'static Item> {
    CATALOG.iter().find(|i| i.id == id)
}

/// Where the browser opens.
#[derive(Clone, Debug, PartialEq)]
pub enum Anchor {
    /// Under a keyed element (the title bar's Create, the Hierarchy's +).
    Key(&'static str),
    /// At a window point, design px (Shift+A at the pointer).
    Point(f32, f32),
}

/// The open browser.
#[derive(Clone, Debug)]
pub struct Browser {
    /// Where it opened.
    pub anchor: Anchor,
    /// The search.
    pub query: String,
    /// The chosen category: `None` is All, `Some(None)` Recent.
    pub category: Option<Option<Category>>,
    /// The highlighted row.
    pub active: usize,
    /// Where new objects go (world), when opened over the Scene view.
    pub place: Option<[f32; 3]>,
    /// Create under the selection.
    pub child: bool,
}

/// Opens the browser at `anchor` (closing it when it is already open there).
pub fn toggle(s: &mut Studio, anchor: Anchor, place: Option<[f32; 3]>) {
    if s.local.create.as_ref().is_some_and(|b| b.anchor == anchor) {
        s.local.create = None;
        return;
    }
    let category = if s.local.recent.is_empty() {
        None
    } else {
        Some(None)
    };
    s.local.create = Some(Browser {
        anchor,
        query: String::new(),
        category,
        active: 0,
        place,
        child: false,
    });
    s.local.focus = Some("create/search");
}

/// Whether the schema has every component `item` adds.
fn available(s: &Studio, item: &Item) -> bool {
    let schema = s.editor.host.store().schema();
    match item.make {
        Make::Group => !s.editor.state.selection.is_empty(),
        Make::Entity { child: true, .. } if s.editor.state.selection.is_empty() => false,
        Make::Entity { components, .. } => serde_json::from_str::<Value>(components)
            .ok()
            .and_then(|v| {
                v.as_object().map(|o| {
                    o.keys()
                        .all(|k| k == "Transform" || schema.get(k).is_some())
                })
            })
            .unwrap_or(false),
    }
}

/// The rows shown: search ranks across everything; otherwise the category's items.
fn shown(s: &Studio, b: &Browser) -> Vec<&'static Item> {
    let query = b.query.trim();
    if !query.is_empty() {
        let mut scored: Vec<(i32, &Item)> = CATALOG
            .iter()
            .filter(|i| available(s, i))
            .filter_map(|i| {
                let best = [
                    super::chrome::score(query, i.title).map(|v| v + 20),
                    super::chrome::score(query, i.keywords),
                    super::chrome::score(query, i.category.title()),
                ]
                .into_iter()
                .flatten()
                .max()?;
                Some((best, i))
            })
            .collect();
        scored.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
        return scored.into_iter().map(|(_, i)| i).collect();
    }
    match b.category {
        Some(None) => s
            .local
            .recent
            .iter()
            .filter_map(|id| item(id))
            .filter(|i| available(s, i))
            .collect(),
        Some(Some(c)) => CATALOG
            .iter()
            .filter(|i| i.category == c && available(s, i))
            .collect(),
        None => CATALOG.iter().filter(|i| available(s, i)).collect(),
    }
}

/// Creates `item` on `target` (at `place` when set; under the target when `child`), selects
/// it and remembers it in Recent.
pub fn create(s: &mut Studio, item: &Item, target: &Target, place: Option<[f32; 3]>, child: bool) {
    let result = match item.make {
        Make::Group => {
            super::commands::run(s, "create.group", target);
            Ok(())
        }
        Make::Entity {
            components,
            child: always_child,
        } => {
            let mut components: Value =
                serde_json::from_str(components).unwrap_or_else(|_| json!({}));
            let under = (child || always_child)
                .then(|| match target {
                    Target::Entity(id) => Some(id.clone()),
                    _ => s.editor.state.selection.first().cloned(),
                })
                .flatten();
            let position = match (place, &under) {
                (_, Some(_)) => None,
                (Some(p), None) => Some(p),
                (None, None) => {
                    let t = s.editor.state.camera.target;
                    components
                        .get("Transform")
                        .and_then(|t| t.get("position"))
                        .is_none()
                        .then_some([t.x, t.y.max(0.0), t.z])
                }
            };
            if components.get("Transform").is_none() {
                components["Transform"] = json!({});
            }
            if let Some(p) = position {
                components["Transform"]["position"] = json!(p);
            }
            let mut args = json!({"name": item.title, "components": components});
            if let Some(parent) = under {
                args["parent"] = parent.into();
            }
            match s.editor.apply(
                vec![json!({"cmd": "entity.create", "args": args})],
                &format!("Create {}", item.title),
                None,
            ) {
                Ok(result) => {
                    s.resolve();
                    if let Some(id) = result["outputs"][0]["id"].as_str() {
                        s.select(vec![id.to_owned()], false);
                    }
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    };
    if let Err(error) = result {
        s.editor.report_failure(&error);
        return;
    }
    s.local.recent.retain(|id| *id != item.id);
    s.local.recent.insert(0, item.id);
    s.local.recent.truncate(8);
}

/// Creates the highlighted row and closes the browser.
fn commit(s: &mut Studio) {
    let Some(b) = s.local.create.clone() else {
        return;
    };
    let rows = shown(s, &b);
    let Some(item) = rows
        .get(b.active.min(rows.len().saturating_sub(1)))
        .copied()
    else {
        return;
    };
    s.local.create = None;
    create(s, item, &Target::None, b.place, b.child);
}

/// The browser's overlay while open.
pub fn overlay(s: &Studio) -> Option<Overlay<Studio>> {
    let b = s.local.create.as_ref()?;
    let panel = panel(s, b);
    let over = popover(panel).on_dismiss(|s: &mut Studio| s.local.create = None);
    Some(match b.anchor {
        Anchor::Key(key) => over.anchor(key),
        Anchor::Point(x, y) => over.at(x, y),
    })
}

fn panel(s: &Studio, b: &Browser) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let rows = shown(s, b);
    let active = b.active.min(rows.len().saturating_sub(1));
    // Categories: Recent (when there is any), All, then each with something to create.
    let searching = !b.query.trim().is_empty();
    let mut cats = col()
        .w(184.0)
        .min_w(184.0)
        .pad(8.0)
        .gap(2.0)
        .scroll(Axis::Vertical);
    let mut cat = |key: String,
                   glyph: Icon,
                   title: &str,
                   count: usize,
                   on: bool,
                   pick: Option<Option<Category>>| {
        let hover = c.hover_surface.0;
        cats = std::mem::replace(&mut cats, col()).child(
            row()
                .key(key)
                .h(28.0)
                .pad_x(8.0)
                .gap(8.0)
                .align(Align::Center)
                .radius(l.radius.sm)
                .fill(if on {
                    c.selection_surface_inactive.0
                } else {
                    Rgba::TRANSPARENT
                })
                .color(if on {
                    c.foreground.0
                } else {
                    c.muted_foreground.0
                })
                .cascade_ink()
                .label(title)
                .selected(on)
                .hovered(move |e| if on { e } else { e.fill(hover) })
                .on_press(move |s: &mut Studio| {
                    if let Some(b) = s.local.create.as_mut() {
                        b.category = pick;
                        b.query.clear();
                        b.active = 0;
                    }
                    s.local.focus = Some("create/search");
                })
                .child(icon(glyph).size(14.0))
                .child(
                    text(title)
                        .text_size(12.5)
                        .weight(if on { 600 } else { 500 })
                        .grow(1.0),
                )
                .child(
                    text(count.to_string())
                        .text_size(11.0)
                        .color(c.muted_foreground.0.with_alpha(0.8)),
                ),
        );
    };
    let total = CATALOG.iter().filter(|i| available(s, i)).count();
    if !s.local.recent.is_empty() {
        cat(
            "create/cat/recent".into(),
            icons::CLOCK,
            "Recent",
            s.local.recent.len(),
            !searching && b.category == Some(None),
            Some(None),
        );
    }
    cat(
        "create/cat/all".into(),
        icons::STAR,
        "All",
        total,
        !searching && b.category.is_none(),
        None,
    );
    for category in Category::ALL {
        let count = CATALOG
            .iter()
            .filter(|i| i.category == category && available(s, i))
            .count();
        if count > 0 {
            cat(
                format!("create/cat/{}", category.title()),
                category.icon(),
                category.title(),
                count,
                !searching && b.category == Some(Some(category)),
                Some(Some(category)),
            );
        }
    }
    // Search: typing filters across everything; arrows move, Enter creates.
    let search = row().pad(10.0).child(
        field(
            l,
            Size::Md,
            "create/search",
            Some(icons::SEARCH),
            "Search everything you can create…",
            &b.query,
            |f| {
                f.on_input(|s: &mut Studio, t| {
                    if let Some(b) = s.local.create.as_mut() {
                        b.query = t.to_owned();
                        b.active = 0;
                    }
                })
                .on_submit(|s: &mut Studio, _| commit(s))
            },
        )
        .grow(1.0),
    );
    let list = if rows.is_empty() {
        col().h(0.0).grow(1.0).justify(Justify::Center).child(empty(
            l,
            icons::SEARCH,
            "Nothing matches",
            "Search looks at names, categories and what objects are for.",
            None,
        ))
    } else {
        let mut list = col()
            .key("create/list")
            .h(0.0)
            .grow(1.0)
            .pad(6.0)
            .gap(1.0)
            .scroll(Axis::Vertical);
        for (i, item) in rows.iter().enumerate() {
            list = list.child(item_row(l, item, i, i == active, searching));
        }
        list
    };
    let right = col()
        .grow(1.0)
        .min_w(0.0)
        .child(search)
        .child(list)
        .child(footer(s, b, rows.get(active).copied()));
    row()
        .key("create")
        .w(700.0)
        .h(460.0)
        .radius(l.radius.md)
        .fill(c.popover.0)
        .shadow_level(stillkit::ShadowLevel::Menu)
        .label("Create")
        .on_key(|s: &mut Studio, key, _| keys(s, key.0))
        .child(cats.fill(c.muted.0.with_alpha(0.5)))
        .child(right)
}

/// Arrow keys and Escape while the browser has focus.
fn keys(s: &mut Studio, key: u16) -> Handled {
    let Some(b) = s.local.create.clone() else {
        return Handled::No;
    };
    let n = shown(s, &b).len();
    let Some(open) = s.local.create.as_mut() else {
        return Handled::No;
    };
    match key {
        0x28 if n > 0 => open.active = (b.active + 1).min(n - 1),
        0x26 => open.active = b.active.saturating_sub(1),
        0x21 => open.active = b.active.saturating_sub(8),
        0x22 if n > 0 => open.active = (b.active + 8).min(n - 1),
        0x1B => s.local.create = None,
        _ => return Handled::No,
    }
    Handled::Yes
}

/// A row: the object's icon on a tinted plate, its name and purpose (and its category when
/// searching).
fn item_row(
    l: &ara_kit::look::Look,
    item: &'static Item,
    at: usize,
    active: bool,
    searching: bool,
) -> El<Studio> {
    let c = &l.c;
    let tint = category_tint(l, item.category);
    let hover = c.hover_surface.0;
    let mut title = row().gap(8.0).align(Align::Center).child(
        text(item.title)
            .text_size(13.0)
            .weight(560)
            .color(c.foreground.0),
    );
    if searching {
        title = title.child(
            text(item.category.title())
                .text_size(11.0)
                .color(c.muted_foreground.0),
        );
    }
    row()
        .key(format!("create/item/{}", item.id))
        .h(48.0)
        .pad_x(8.0)
        .gap(12.0)
        .align(Align::Center)
        .radius(l.radius.sm)
        .fill(if active {
            c.selection_surface_inactive.0
        } else {
            Rgba::TRANSPARENT
        })
        .border(
            1.0,
            if active {
                c.ring.0.with_alpha(0.45)
            } else {
                Rgba::TRANSPARENT
            },
        )
        .label(item.title)
        .description(item.blurb)
        .selected(active)
        .hovered(move |e| if active { e } else { e.fill(hover) })
        .on_pointer(move |s: &mut Studio, e, _| match e.phase {
            stillkit::el::PointerPhase::Hover => {
                if let Some(b) = s.local.create.as_mut()
                    && b.active != at
                {
                    b.active = at;
                }
                false
            }
            stillkit::el::PointerPhase::Up if e.button == stillkit::Button::Left => {
                if let Some(b) = s.local.create.as_mut() {
                    b.active = at;
                }
                commit(s);
                true
            }
            stillkit::el::PointerPhase::Down => true,
            _ => false,
        })
        .child(
            el().size(32.0)
                .radius(8.0)
                .fill(tint.with_alpha(0.16))
                .border(1.0, tint.with_alpha(0.28))
                .align(Align::Center)
                .justify(Justify::Center)
                .child(icon(item.icon).size(16.0).color(tint)),
        )
        .child(
            col()
                .grow(1.0)
                .min_w(0.0)
                .gap(2.0)
                .child(title)
                .child(text(item.blurb).text_size(11.5).color(c.muted_foreground.0)),
        )
}

/// Each category's colour, from the theme's chart palette.
fn category_tint(l: &ara_kit::look::Look, category: Category) -> Rgba {
    let c = &l.c;
    match category {
        Category::Basic => c.muted_foreground.0,
        Category::Shapes | Category::TwoD => c.chart_1.0,
        Category::Lights | Category::Post => c.chart_4.0,
        Category::Camera | Category::Audio => c.chart_2.0,
        Category::Effects => c.chart_5.0,
        Category::Physics | Category::Navigation => c.chart_3.0,
        Category::Gameplay => c.chart_6.0,
    }
}

/// The highlighted object's components and where it goes; Create.
fn footer(s: &Studio, b: &Browser, item: Option<&'static Item>) -> El<Studio> {
    let l = &s.look;
    let c = &l.c;
    let mut out = row().h(48.0).pad_x(12.0).gap(8.0).align(Align::Center);
    let Some(item) = item else { return out };
    let adds: Vec<String> = match item.make {
        Make::Entity { components, .. } => serde_json::from_str::<Value>(components)
            .ok()
            .and_then(|v| {
                v.as_object()
                    .map(|o| o.keys().filter(|k| *k != "Transform").cloned().collect())
            })
            .unwrap_or_default(),
        Make::Group => Vec::new(),
    };
    out = out.child(text("Adds").text_size(11.5).color(c.muted_foreground.0));
    if adds.is_empty() {
        out = out.child(chip(l, Status::Neutral, "Transform", false));
    }
    for name in adds.iter().take(3) {
        out = out.child(chip(l, Status::Info, name, false));
    }
    out = out.child(el().grow(1.0));
    // Where it goes: the pointer's ground point, or under the selection.
    let selected = s.editor.state.selection.first().map(|id| s.name(id));
    if let (Some(name), Make::Entity { child: false, .. }) = (selected, item.make) {
        let on = b.child;
        out = out.child(
            checkbox(l, "create/child", &format!("Inside {name}"), on).on_press(
                move |s: &mut Studio| {
                    if let Some(b) = s.local.create.as_mut() {
                        b.child = !on;
                    }
                },
            ),
        );
    } else if b.place.is_some() {
        out = out.child(
            text("At the pointer")
                .text_size(11.5)
                .color(c.muted_foreground.0),
        );
    }
    out.child(
        button(l, Tone::Primary, Size::Sm, Some(icons::PLUS), "Create")
            .key("create/go")
            .on_press(commit),
    )
}
