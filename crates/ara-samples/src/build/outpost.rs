//! "Outpost at dusk": a derelict compound in arid Namaqualand-style scrubland. A
//! 384 m terrain (12 by 12 tiles of 32 m) with hills at the rim; in the middle a concrete
//! pad inside a chain-link perimeter with a hangar, a guard hut at the gate, stacked
//! shipping containers, a radio mast, a covered car, crates and barrels, a campfire
//! and floodlights. A dirt road leaves through the gate past electricity poles.
//! The sun sets in the west (−X). The player is the CC0 Mesh2Motion mannequin.
use super::{
    Ctx, anim, clean,
    kits::{self, Hangar},
    place::{Ground, Models, Prop, Scene, create, transform},
};
use crate::{
    kit::{self, Kit, Material, meters, part},
    lock::Lock,
};
use anyhow::Result;
use glam::{Vec2, Vec3};
use serde_json::{Value, json};
use std::path::Path;

#[cfg(test)]
mod audit;

/// Compound centre in world X/Z metres.
const C: Vec2 = Vec2::new(96.0, 96.0);
/// The complete resident grid along each axis (32 metres per tile).
const TERRAIN_TILES: i32 = 12;
/// Level scene.
pub const SCENE: (&str, &str) = ("levels/outpost.scene", "sc_outpost1");
const TERRAIN: (&str, &str) = ("terrain/outpost.terrain", "tr_outpost1");
const FOLIAGE: (&str, &str) = ("terrain/outpost.foliage", "fo_outpost1");
const ANIMATION: (&str, &str) = ("anim/human.animation", "ac_human1");
const MUSIC: (&str, &str) = ("audio/outpost.music", "mu_outpost1");
/// The hangar centre (its door faces −Z, toward the pad).
pub const HANGAR: Vec3 = Vec3::new(96.0, 0.0, 117.0);
/// Where the player starts (by the campfire).
const START: Vec3 = Vec3::new(86.5, 0.05, 89.0);

fn reference((path, id): (&str, &str)) -> String {
    format!("@{path}#{id}")
}

/// Builds the whole project: shared assets, the dusk exterior and the night hangar.
pub fn build(ctx: &Ctx) -> Result<()> {
    clean(
        ctx,
        &[
            "levels",
            "prefabs",
            "materials",
            "terrain",
            "anim",
            "audio",
            "assets/kit",
            ".scratch",
        ],
    )?;
    let models = models(ctx)?;
    terrain(ctx)?;
    let ground = Ground::load(&ctx.root, TERRAIN.0)?;
    let mut models = models;
    road(ctx, &mut models, &ground)?;
    foliage(ctx, &models)?;
    audio(ctx)?;
    let human = ctx.lock.get("m2m_human").expect("m2m_human pinned");
    anim::locomotion(
        ctx,
        ANIMATION.0,
        ANIMATION.1,
        "an_locomotion",
        &human.dir(&ctx.root).join("human-base-animations.glb"),
        "Idle_A",
        &[
            anim::Gait {
                clip: "Walk",
                speed: 1.5,
            },
            anim::Gait {
                clip: "Jog",
                speed: 3.2,
            },
            anim::Gait {
                clip: "Sprint",
                speed: 5.5,
            },
        ],
    )?;
    exterior(ctx, &models, &ground)?;
    super::hangar::build(ctx, &models)?;
    ctx.set_main(SCENE.0, SCENE.1)?;
    ctx.check()?;
    Ok(())
}

/// Original source models, native prefabs of selected kit nodes, and generated geometry.
fn models(ctx: &Ctx) -> Result<Models> {
    let mut m = Models::default();
    for (name, asset) in [
        ("barrel_blue", "barrel_01"),
        ("barrel_red", "barrel_02"),
        ("barrel_rust", "barrel_03"),
        ("barrel_stove", "barrel_stove"),
        ("crate_long", "wooden_crate_02"),
        ("crate_flat", "wooden_crate_01"),
        ("crate_military", "wooden_military_crate"),
        ("crate_plastic", "plastic_crate_03"),
        ("box", "cardboard_box_01"),
        ("jerrycan", "metal_jerrycan"),
        ("jerrycan_green", "metal_jerrycan_green"),
        ("propane", "propane_tank"),
        ("lpg", "small_lpg_tank"),
        ("barrier", "concrete_road_barrier"),
        ("barrier_tall", "concrete_road_barrier_02"),
        ("car", "covered_car"),
        ("tyre", "old_tyre"),
        ("rim", "rusted_wheel_rim_01"),
        ("searchlight", "portable_searchlight"),
        ("utility_box", "utility_box_01"),
        ("utility_box_wide", "utility_box_02"),
        ("generator", "portable_generator"),
        ("compressor", "old_military_compressor"),
        ("fire_pit", "stone_fire_pit"),
        ("trashbag", "trashbag"),
        ("hand_truck", "hand_truck"),
        ("ladder", "wooden_ladder"),
        ("boulder_flat", "namaqualand_boulder_02"),
        ("boulder", "namaqualand_boulder_03"),
        ("boulder_big", "namaqualand_boulder_04"),
        ("boulder_small", "namaqualand_boulder_05"),
        ("quiver_tree", "quiver_tree_01"),
        ("quiver_tree_young", "quiver_tree_02"),
        ("dead_quiver", "dead_quiver_trunk"),
        ("quiver_branch", "dead_quiver_branch_01"),
        ("log", "dead_tree_trunk"),
        ("radio", "vintage_radio_transceiver"),
        ("gas_mask", "old_gas_mask"),
        // Hangar interior.
        ("shelves", "steel_frame_shelves_03"),
        ("tool_chest", "metal_tool_chest"),
        ("tool_cart", "tool_cart"),
        ("storage_cart", "industrial_storage_cart"),
        ("welding_cart", "portable_welding_cart"),
        ("drill_press", "drill_press_01"),
        ("bench_vice", "bench_vice_01"),
        ("workbench", "woodentable_03"),
        ("desk", "metal_office_desk"),
        ("rack", "worn_metal_rack"),
        ("crane", "overhead_crane"),
        ("hanging_lamp", "hanging_industrial_lamp"),
        ("pipes", "modular_industrial_pipes_01"),
        ("power_box", "power_box_01"),
        ("wrench", "adjustable_wrench"),
        ("pipe_wrench", "pipe_wrench"),
        ("crowbar", "crowbar_01"),
        ("sledgehammer", "sledgehammer_01"),
        ("toolbox", "metal_toolbox"),
        ("ladder_tall", "ladder_sectioned_01"),
        ("old_drill_press", "old_drill_press"),
        ("multimeter", "retro_multimeter"),
        ("laptop", "classic_laptop"),
        ("stool", "metal_stool_01"),
        ("oil_can", "small_oil_can_01"),
        ("spray_paint", "spray_paint_bottles_02"),
        ("hammer", "wooden_hammer_01"),
    ] {
        ctx.third_party(&mut m, name, asset)?;
    }
    // Variants cut from sheets: (name, asset, parts).
    let p = |n: &'static str| part(n, [0.0; 3], 0.0);
    let variants: Vec<(&str, &str, Vec<kit::Part<'static>>)> = vec![
        (
            "aircon",
            "exterior_aircon_unit",
            vec![p("exterior_aircon_unit_rusted")],
        ),
        (
            "trash_can",
            "metal_trash_can",
            vec![
                p("metal_trash_can_rust"),
                part("metal_trash_can_rust_handle_left", [-0.27, 0.65, 0.0], 0.0),
                part("metal_trash_can_rust_handle_right", [0.27, 0.65, 0.0], 0.0),
            ],
        ),
        (
            "ammo_crate",
            "old_military_crate",
            vec![
                p("old_military_crate_a"),
                part("old_military_crate_lid_a", [0.15, 0.25, -0.22], 0.0),
                part("old_military_crate_latch_a", [0.0, 0.23, 0.23], 0.0),
                part("old_military_crate_loop_a", [0.0, 0.21, 0.22], 0.0),
            ],
        ),
        (
            "shutter",
            "rollershutter_door",
            vec![p("rollershutter_door_graffiti")],
        ),
        (
            "fluorescent",
            "mounted_fluorescent_lights",
            vec![p("mounted_fluorescent_lights_a")],
        ),
        (
            "sconce",
            "industrial_caged_sconce",
            vec![p("industrial_caged_sconce_c")],
        ),
        (
            "duct",
            "modular_airduct_rectangular_01",
            vec![p("modular_airduct_rectangular_01_tripple_01")],
        ),
        (
            "rooibos_a",
            "wild_rooibos_bush",
            vec![p("wild_rooibos_bush_a")],
        ),
        (
            "rooibos_c",
            "wild_rooibos_bush",
            vec![p("wild_rooibos_bush_c")],
        ),
        ("shrub_a", "shrub_03", vec![p("shrub_03_a")]),
        ("shrub_b", "shrub_03", vec![p("shrub_03_b")]),
        (
            "branches",
            "dry_branches_medium_01",
            vec![p("dry_branches_medium_01_a")],
        ),
        (
            "rock_a",
            "namaqualand_rocks_01",
            vec![p("namaqualand_rocks_02_a")],
        ),
        (
            "rock_d",
            "namaqualand_rocks_01",
            vec![p("namaqualand_rocks_02_d")],
        ),
        (
            "iceplant",
            "crystalline_iceplant",
            vec![
                p("crystalline_iceplant_a_LOD0"),
                part("crystalline_iceplant_c_LOD0", [0.35, 0.0, 0.2], 40.0),
            ],
        ),
        // A 7 m utility pole with a cross-arm and insulators.
        (
            "pole",
            "modular_electricity_poles",
            vec![
                p("pole_large"),
                part("support_large_01", [0.0, 6.2, 0.0], 0.0),
                part("insulators_large", [0.0, 6.45, 0.0], 0.0),
                part("fuse_01", [0.0, 5.2, 0.12], 0.0),
            ],
        ),
    ];
    for (name, asset, parts) in &variants {
        let lit: &[&str] = match *name {
            "fluorescent" => &["mounted_fluorescent_lights_glass"],
            "sconce" => &["industrial_caged_sconce"],
            _ => &[],
        };
        super::native::parts(ctx, &mut m, name, asset, parts, lit)?;
    }
    human(ctx, &mut m)?;
    // Lamps, with their bulbs and lenses switched on (the sources ship them off).
    for (name, asset, lit) in [
        (
            "floodlight",
            "street_lamp_01",
            &["street_lamp_01_bulb", "street_lamp_01_glass"][..],
        ),
        (
            "wall_lamp",
            "street_lamp_02",
            &["street_lamp_02_bulb", "street_lamp_02_glass"][..],
        ),
        (
            "security_light",
            "security_light",
            &["security_light_bulb", "security_light_glass"][..],
        ),
        (
            "caged_light",
            "caged_hanging_light",
            &["caged_hanging_light"][..],
        ),
    ] {
        super::native::whole(ctx, &mut m, name, asset, lit)?;
    }
    // Generated kit pieces.
    super::native::parts(
        ctx,
        &mut m,
        "fence_section",
        "modular_chainlink_fence",
        &[
            part("modular_chainlink_fence_double", [2., 0., 0.], 0.),
            part("modular_chainlink_fence_post", [0., 0., 0.], 0.),
        ],
        &[],
    )?;
    super::native::parts(
        ctx,
        &mut m,
        "fence_gate",
        "modular_chainlink_fence",
        &[
            part("modular_chainlink_fence_double", [2., 0., 0.], 0.),
            part("modular_chainlink_fence_post", [0., 0., 0.], 0.),
            part("modular_chainlink_fence_post", [2., 0., 0.], 0.),
        ],
        &[],
    )?;
    for (name, path) in kits(&ctx.root, &ctx.lock)? {
        ctx.model(
            &mut m,
            name,
            &path,
            &format!("as_kit{}", super::place::short(name)),
        )?;
    }
    Ok(m)
}

/// Writes the generated kit models the levels place (road and suit palette aside)
/// and returns `(model name, glTF path)`. Only ignored `assets/kit/` files change.
fn kits(root: &Path, lock: &Lock) -> Result<Vec<(&'static str, String)>> {
    Ok(vec![
        (
            "container_green",
            kits::container(
                root,
                lock,
                "container_green",
                "container_side",
                [0.6, 0.68, 0.62],
            )?,
        ),
        (
            "container_rust",
            kits::container(
                root,
                lock,
                "container_rust",
                "rusty_corrugated_iron",
                [1.0, 1.0, 1.0],
            )?,
        ),
        (
            "container_blue",
            kits::container(
                root,
                lock,
                "container_blue",
                "container_side",
                [0.42, 0.55, 0.68],
            )?,
        ),
        ("guard_hut", kits::guard_hut(root, lock, "guard_hut")?),
        ("hangar", Hangar::OUTPOST.write(root, lock, "hangar")?),
        ("mast", kits::mast(root, lock, "mast", 20.0)?),
        (
            "pad",
            kits::slab(
                root,
                lock,
                "pad",
                "concrete_floor_worn_001",
                Vec3::new(48.0, 0.3, 30.0),
            )?,
        ),
        (
            "apron",
            kits::slab(
                root,
                lock,
                "apron",
                "concrete_block_wall",
                Vec3::new(4.0, 0.2, 4.0),
            )?,
        ),
        ("puddle_a", kits::puddle(root, lock, "puddle_a", 1.6, 3)?),
        ("puddle_b", kits::puddle(root, lock, "puddle_b", 1.1, 11)?),
        ("puddle_c", kits::puddle(root, lock, "puddle_c", 2.4, 7)?),
        (
            "light_panel",
            kits::light_panel(
                root,
                lock,
                "light_panel",
                Vec2::new(1.2, 0.3),
                [1.0, 0.96, 0.9],
            )?,
        ),
        (
            "window",
            kits::window(root, lock, "window", Vec2::new(1.4, 1.0))?,
        ),
        ("festoon", super::hangar::festoon(root, lock)?),
        ("embers", kits::embers(root, lock, "embers")?),
        ("cables", super::hangar::cables(root, lock)?),
    ])
}

/// Writes every generated, git-ignored Outpost file (`assets/kit/`) without
/// authoring a document. `ara-samples kits` and release packaging run this on a
/// project whose third-party assets are fetched.
pub fn generate(root: &Path, lock: &Lock) -> Result<()> {
    kits(root, lock)?;
    road_kit(root, lock, &Ground::load(root, TERRAIN.0)?)?;
    suit(root, lock)?;
    Ok(())
}

/// The CC0 mannequin with its colour palette recast as a graphite work suit.
pub fn human(ctx: &Ctx, models: &mut Models) -> Result<()> {
    let palette = suit(&ctx.root, &ctx.lock)?;
    super::materials::palette(ctx, models, "human", "m2m_human", &palette)
}

/// Writes `assets/kit/human_suit.png`, the recoloured mannequin palette.
pub fn suit(root: &Path, lock: &Lock) -> Result<String> {
    kit::recolor(
        root,
        lock,
        "m2m_human",
        "color-palette",
        [0.52, 0.54, 0.56],
        "human_suit",
    )
}

/// Sculpts the terrain: flat ground under the compound, rolling scrubland, hills at the
/// rim. Paints soil everywhere, rock on the hills, sparse dry grass patches.
fn terrain(ctx: &Ctx) -> Result<()> {
    let materials = vec![
        super::materials::terrain(ctx, "groundgrass", "dry_ground_01", [0.72, 0.78, 0.6])?,
        super::materials::terrain(ctx, "groundrock", "rocky_terrain_02", [0.9, 0.86, 0.82])?,
        super::materials::terrain(ctx, "groundsoil", "dry_ground_rocks", [0.86, 0.8, 0.74])?,
    ];
    ctx.new_document(TERRAIN.0, "terrain", TERRAIN.1)?;
    let dab = |x: f32, z: f32, radius: f32, amount: f32| json!({"center": [x, z], "radius": radius, "amount": amount});
    // Deterministic jitter for the mountain ring and ground bumps.
    let mut seed = 0x9e37_79b9u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let mut mountains = Vec::new();
    for i in 0..28 {
        let a = i as f32 / 28.0 * std::f32::consts::TAU + rand() * 0.15;
        // A pass to the south (−Z) lets the road out of the basin.
        let pass = (a - 1.5 * std::f32::consts::PI).abs() < 0.28;
        let r = 172.0 + rand() * 18.0;
        let height = if pass { 6.0 } else { 26.0 + rand() * 30.0 };
        mountains.push((
            C.x + a.cos() * r,
            C.y + a.sin() * r,
            34.0 + rand() * 16.0,
            height,
        ));
    }
    let mut bumps = Vec::new();
    for _ in 0..160 {
        let (x, z) = (C.x - 150.0 + rand() * 300.0, C.y - 150.0 + rand() * 300.0);
        if (Vec2::new(x, z) - C).length() > 46.0 {
            bumps.push((x, z, 3.0 + rand() * 7.0, (rand() - 0.45) * 0.9));
        }
    }
    for &(x, z, r, h) in &mountains.clone() {
        if h > 10.0 {
            // Secondary peaks, gullies running down toward the basin, fine roughness.
            for _ in 0..5 {
                mountains.push((
                    x + (rand() - 0.5) * 50.0,
                    z + (rand() - 0.5) * 50.0,
                    8.0 + rand() * 10.0,
                    (rand() - 0.3) * 6.0,
                ));
            }
            let inward = (C - Vec2::new(x, z)).normalize();
            for g in 0..6 {
                let side = Vec2::new(-inward.y, inward.x) * (g as f32 - 2.5) * r * 0.28;
                let along = inward * r * (0.35 + rand() * 0.45);
                let p = Vec2::new(x, z) + side + along;
                mountains.push((p.x, p.y, 4.0 + rand() * 4.0, -(2.0 + rand() * 4.0)));
            }
            for _ in 0..8 {
                let p = Vec2::new(x, z) + Vec2::new(rand() - 0.5, rand() - 0.5) * r * 1.6;
                mountains.push((p.x, p.y, 3.0 + rand() * 4.0, (rand() - 0.5) * 3.0));
            }
        }
    }
    let hills = [
        (14.0, 30.0, 30.0, 5.0),
        (8.0, 110.0, 34.0, 6.0),
        (36.0, 178.0, 30.0, 10.0),
        (110.0, 186.0, 34.0, 15.0),
        (176.0, 160.0, 30.0, 11.0),
        (186.0, 90.0, 28.0, 8.0),
        (168.0, 18.0, 30.0, 9.0),
        (60.0, 6.0, 24.0, 5.0),
        (150.0, 120.0, 14.0, 3.5),
        (46.0, 140.0, 16.0, 4.0),
    ];
    let swells = [
        (140.0, 60.0, 14.0, 1.2),
        (52.0, 64.0, 12.0, 1.0),
        (70.0, 156.0, 12.0, 1.4),
        (128.0, 156.0, 10.0, 0.8),
        (150.0, 92.0, 10.0, -0.6),
        (40.0, 96.0, 10.0, -0.5),
    ];
    let mut raise: Vec<Value> = hills.iter().map(|&(x, z, r, a)| dab(x, z, r, a)).collect();
    raise.extend(swells.iter().map(|&(x, z, r, a)| dab(x, z, r, a)));
    raise.extend(bumps.iter().map(|&(x, z, r, a)| dab(x, z, r, a)));
    let peaks: Vec<Value> = mountains
        .iter()
        .map(|&(x, z, r, a)| dab(x, z, r, a))
        .collect();
    let mut rock: Vec<Value> = hills
        .iter()
        .filter(|h| h.3 > 6.0)
        .map(|&(x, z, r, _)| dab(x, z, r * 0.45, 0.9))
        .collect();
    rock.extend(
        mountains
            .iter()
            .filter(|m| m.3 > 10.0)
            .map(|&(x, z, r, _)| dab(x, z, r * 0.9, 1.0)),
    );
    let grass = [
        (60.0, 120.0),
        (132.0, 70.0),
        (140.0, 140.0),
        (54.0, 44.0),
        (120.0, 30.0),
    ]
    .iter()
    .map(|&(x, z)| dab(x, z, 9.0, 0.5))
    .collect::<Vec<_>>();
    ctx.apply(
        TERRAIN.0,
        "Outpost terrain: flat compound ground, rolling scrubland, foothills and a rocky mountain rim",
        vec![
            (
                "terrain.create",
                json!({"grid": {"origin": [-96, -96], "spacing": 1, "cells": 32,
                    "min": {"x": 0, "z": 0}, "max": {"x": TERRAIN_TILES, "z": TERRAIN_TILES}, "materials": materials}}),
            ),
            (
                "terrain.layer.add",
                json!({"id": "l_sculpt", "layer": {"name": "Sculpt", "priority": 0, "enabled": true, "opacity": 1}}),
            ),
            ("terrain.raise", json!({"id":"s_ground", "layer": "l_sculpt", "dabs": raise})),
            ("terrain.raise", json!({"id":"s_peaks", "layer": "l_sculpt", "dabs": peaks})),
            ("terrain.paint", json!({"id":"s_soil", "layer": "l_sculpt", "channel": 2, "dabs": [dab(96.0, 96.0, 320.0, 1.0)]})),
            ("terrain.paint", json!({"id":"s_rock", "layer": "l_sculpt", "channel": 1, "dabs": rock})),
            ("terrain.paint", json!({"id":"s_grass", "layer": "l_sculpt", "channel": 0, "dabs": grass})),
        ],
    )
}

/// The dirt road: from the gate south and out of the valley, a ground-hugging strip.
fn road_path() -> Vec<Vec2> {
    vec![
        Vec2::new(96.0, 79.0),
        Vec2::new(96.5, 66.0),
        Vec2::new(99.0, 52.0),
        Vec2::new(104.0, 38.0),
        Vec2::new(107.0, 24.0),
        Vec2::new(106.0, 10.0),
        Vec2::new(101.0, -14.0),
        Vec2::new(97.0, -40.0),
        Vec2::new(96.0, -72.0),
    ]
}

fn road(ctx: &Ctx, models: &mut Models, ground: &Ground) -> Result<()> {
    let path = road_kit(&ctx.root, &ctx.lock, ground)?;
    ctx.model(models, "road", &path, "as_kitroad")
}

/// Writes the road's glTF: a strip hugging the evaluated terrain.
fn road_kit(root: &Path, lock: &Lock, ground: &Ground) -> Result<String> {
    let set = "rocky_trail";
    let mut kit = Kit::new(vec![
        Material::textured("trail", set).tinted([0.9, 0.76, 0.64]),
    ]);
    kit.ribbon(0, &road_path(), 5.0, 0.03, meters(lock, set), &|x, z| {
        ground.height(x, z)
    });
    kit.write(root, lock, "road")
}

/// Scatter rules: dry shrubs and branches over the scrub, small rocks on the slopes;
/// none in the compound or on the road.
fn foliage(ctx: &Ctx, models: &Models) -> Result<()> {
    ctx.new_document(FOLIAGE.0, "foliage", FOLIAGE.1)?;
    let rule = |name: &str,
                species: &str,
                model: &str,
                seed: u64,
                cell: f32,
                density: f32,
                slope: [f32; 2],
                channel: u32,
                scale: [f32; 2]| {
        json!({"name": name, "species": species, "asset": models.get(model).reference, "seed": seed,
            "cellSize": cell, "density": density, "height": [-5.0, 14.0], "slope": slope,
            "channel": channel, "minWeight": 0.3, "noise": 0.6, "scale": scale, "enabled": true})
    };
    let mut calls = vec![
        (
            "foliage.create",
            json!({"header": {"terrain": reference(TERRAIN)}}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_shrub", "rule": rule("Dry shrub", "grass", "shrub_a", 21, 2.0, 0.05, [0.0, 35.0], 2, [1.4, 2.6])}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_shrubb", "rule": rule("Dry shrub (sparse)", "grass", "shrub_b", 22, 3.0, 0.03, [0.0, 35.0], 2, [1.6, 2.8])}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_rooibos", "rule": rule("Rooibos bush", "grass", "rooibos_c", 23, 5.0, 0.02, [0.0, 30.0], 2, [1.2, 2.2])}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_branch", "rule": rule("Dead branches", "rock", "branches", 24, 8.0, 0.008, [0.0, 25.0], 2, [0.8, 1.3])}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_rocks", "rule": rule("Scree", "rock", "rock_a", 25, 4.0, 0.025, [6.0, 60.0], 1, [1.5, 4.0])}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_stones", "rule": rule("Stones", "rock", "rock_d", 26, 4.0, 0.02, [0.0, 60.0], 2, [1.0, 2.5])}),
        ),
        (
            "foliage.rule.set",
            json!({"id": "r_iceplant", "rule": rule("Ice plant", "grass", "iceplant", 27, 2.0, 0.08, [0.0, 25.0], 0, [0.8, 1.6])}),
        ),
    ];
    let mut zones = vec![(C.x, C.y + 4.0, 37.0)];
    zones.extend(road_path().iter().map(|p| (p.x, p.y, 5.0)));
    for (i, (x, z, r)) in zones.into_iter().enumerate() {
        calls.push((
            "foliage.exclude",
            json!({"id":format!("x_clear{i}"), "zone": {"center": [x, z], "radius": r}}),
        ));
    }
    ctx.apply(
        FOLIAGE.0,
        "Arid scrub: shrubs, dead branches and scree, clear of the compound and road",
        calls,
    )
}

/// Audio sidecars and the music score.
fn audio(ctx: &Ctx) -> Result<()> {
    let sfx = json!({"usage": "shortEffect", "codec": "pcm"});
    for (asset, file, id, settings) in [
        ("oga_wind", "wind_loop.ogg", "as_wind", sfx.clone()),
        ("oga_fireplace", "fire.wav", "as_fire", sfx.clone()),
        ("oga_freezer", "freezer.ogg", "as_drone", sfx.clone()),
        (
            "oga_lonelyhood",
            "Lonelyhood.ogg",
            "as_lonelyhood",
            json!({"usage": "music"}),
        ),
    ] {
        let entry = ctx.lock.get(asset).expect("audio pinned");
        ctx.sidecar(&entry.relative(file), id, settings)?;
    }
    music(ctx)
}

/// One looping segment of the CC0 "Calm Track", authored through music commands.
fn music(ctx: &Ctx) -> Result<()> {
    let track = ctx.lock.get("oga_lonelyhood").expect("music pinned");
    ctx.new_document(MUSIC.0, "music", MUSIC.1)?;
    ctx.apply(MUSIC.0, "Compose the dusk music score", vec![
        ("music.settings.set", json!({"settings": {"initial": "dusk", "intensity": 0.5, "seed": 7}})),
        ("music.segment.set", json!({"id": "sg_dusk", "segment": {
            "name": "dusk", "bars": 64, "tempo": [{"bpm": 60}],
            "stems": [{"name": "lonelyhood", "clip": format!("@{}#as_lonelyhood", track.relative("Lonelyhood.ogg")), "gainDb": -10}]
        }})),
    ])
}

/// A light entity.
pub fn light(
    id: &str,
    name: &str,
    position: Vec3,
    light: Value,
    direction: Option<Vec3>,
) -> Vec<(&'static str, Value)> {
    let mut calls = vec![create(
        id,
        name,
        json!({"Transform":transform(position,0.,1.),"Light":light}),
    )];
    if let Some(direction) = direction {
        calls.push((
            "transform.lookAt",
            json!({"entity":id,"point":(position+direction).to_array()}),
        ));
    }
    calls
}

/// The dusk exterior.
fn exterior(ctx: &Ctx, models: &Models, ground: &Ground) -> Result<()> {
    ctx.new_document(SCENE.0, "scene", SCENE.1)?;
    let mut s = Scene::new(models);
    let g = |x: f32, z: f32| ground.at(x, z);
    let pad_top = 0.05;
    let on_pad = |x: f32, z: f32| Vec3::new(x, pad_top, z);

    // Sky, sun, terrain, foliage, camera, player.
    s.entity(
        "e_env",
        "Dusk sky",
        json!({
            "Environment": {"sky": [0.16, 0.22, 0.42, 1], "horizon": [1.0, 0.56, 0.32, 1], "ground": [0.16, 0.12, 0.09, 1], "intensity": 420, "exposure": 0.0028},
            "SkyAtmosphere": {"aerosols": 1.5, "cloudCoverage": 0.12},
            "HeightFog": {"density": 0.0012, "falloff": 30, "albedo": [0.92, 0.86, 0.78, 1], "anisotropy": 0.5},
            "PostProcess": {"compensation": -0.5, "bloom": 0.05}
        }),
    );
    let sun_toward = Vec3::new(-0.9302, 0.0698, 0.3601).normalize();
    s.commands.extend(light(
        "e_sun",
        "Setting sun",
        Vec3::new(96.0, 40.0, 96.0),
        json!({"kind": "directional", "color": [1.0, 0.958, 0.896, 1], "intensity": 24000}),
        Some(-sun_toward),
    ));
    s.entity(
        "e_terrain",
        "Terrain",
        json!({"Transform": {}, "Terrain": {"terrain": reference(TERRAIN), "foliage": reference(FOLIAGE)}}),
    );
    // This level is not streamed: native presentation follows chunk residency,
    // including the road and mountain rim outside the walkable compound.
    for z in 0..TERRAIN_TILES {
        for x in 0..TERRAIN_TILES {
            s.entity(
                &format!("e_chunk{x}z{z}"),
                "Terrain chunk",
                json!({"TerrainChunk": {"x": x, "z": z}, "AcousticSurface": {"preset": "gravel"}}),
            );
        }
    }
    s.entity(
        "e_humankind",
        "Human agents",
        json!({"NavAgentType": {"name": "Human", "height": 1.8, "radius": 0.3, "slope": 40}}),
    );
    s.entity(
        "e_player",
        "Salvager",
        json!({
            "Transform": transform(START, 140.0, 1.0),
            "MeshRenderer": models.get("human").renderer,
            "Animator": {"controller": reference((ANIMATION.0, "an_locomotion"))},
            "CharacterController": {},
            "PlayerControl": {"yaw": 140, "camera": "e_camera", "walkSpeed": 1.5, "runSpeed": 5.0},
            "AudioListener": {},
        }),
    );
    s.entity(
        "e_camera",
        "Follow camera",
        json!({"Transform": {"position": [84.0, 2.2, 86.0]}, "Camera": {"fov": 55},
            "FollowCamera": {"target": "e_player", "distance": 4.2, "height": 1.55, "yaw": 140, "pitch": 12}}),
    );
    s.entity(
        "e_navsurface",
        "Compound navigation",
        json!({"Transform": {"position": [C.x, 0.0, C.y]}, "NavMeshSurface": {"min": [C.x - 35.0, -5.0, C.y - 35.0], "max": [C.x + 35.0, 8.0, C.y + 35.0]}}),
    );

    // Ground works: pad, hangar, apron slabs, road, puddles.
    s.prop(
        Prop::new("e_pad", "Concrete pad", "pad", on_pad(96.0, 92.0), 0.0)
            .at_origin()
            .decor(),
    );
    s.entity(
        "e_padc",
        "Pad collider",
        json!({"Transform": {"position": [96.0, -0.1, 92.0]}, "RigidBody": {}, "Collider": {"halfExtents": [24.0, 0.15, 15.0]}}),
    );
    s.prop(
        Prop::new("e_hangar", "Hangar", "hangar", HANGAR, 0.0)
            .at_origin()
            .decor(),
    );
    hangar_colliders_at(&mut s, HANGAR);
    s.prop(
        Prop::new("e_road", "Dirt road", "road", Vec3::ZERO, 0.0)
            .at_origin()
            .decor(),
    );
    for (i, (name, x, z, yaw)) in [
        ("puddle_a", 101.8, 44.0, 70.0),
        ("puddle_b", 99.0, 84.0, 0.0),
        ("puddle_a", 106.0, 101.0, 130.0),
    ]
    .into_iter()
    .enumerate()
    {
        let y = if z > 78.0 {
            pad_top
        } else {
            ground.height(x, z) + 0.03
        };
        s.prop(
            Prop::new(
                &format!("e_puddle{i}"),
                "Puddle",
                name,
                Vec3::new(x, y, z),
                yaw,
            )
            .at_origin()
            .decor(),
        );
    }

    // Perimeter fence: x 64..128, z 72..134, gate gap at x 92..100 on the south side.
    fence(&mut s, ground);

    // Gate: barriers, guard hut, a tall barrier pair.
    for (i, (x, yaw)) in [(91.0, 8.0), (101.2, -6.0)].into_iter().enumerate() {
        s.prop(Prop::new(
            &format!("e_barrier{i}"),
            "Road barrier",
            "barrier",
            g(x, 71.0),
            yaw,
        ));
    }
    s.prop(Prop::new(
        "e_barrier2",
        "Road barrier",
        "barrier_tall",
        g(103.4, 72.6),
        75.0,
    ));
    s.prop(Prop::new("e_hut", "Guard hut", "guard_hut", on_pad(86.0, 80.5), 180.0).at_origin());
    s.commands.extend(light(
        "e_hutlight",
        "Guard hut lamp",
        Vec3::new(86.0, 2.3, 80.5),
        json!({"color": [1.0, 0.72, 0.42, 1], "intensity": 900, "range": 7}),
        None,
    ));

    // Hangar apron: the covered car, tyres, a trash can, the shutter on the side annex.
    s.prop(Prop::new(
        "e_car",
        "Covered car",
        "car",
        on_pad(104.5, 96.5),
        196.0,
    ));
    for (i, (x, z, yaw)) in [(108.6, 99.0, 10.0), (109.1, 99.6, 40.0)]
        .into_iter()
        .enumerate()
    {
        s.prop(Prop::new(&format!("e_tyre{i}"), "Old tyre", "tyre", on_pad(x, z), yaw).decor());
    }
    s.prop(Prop::new("e_rim0", "Wheel rim", "rim", on_pad(107.5, 98.2), 70.0).decor());
    s.prop(Prop::new(
        "e_trashcan",
        "Trash can",
        "trash_can",
        on_pad(89.2, 103.0),
        15.0,
    ));
    s.prop(
        Prop::new(
            "e_trashbag0",
            "Trash bags",
            "trashbag",
            on_pad(88.4, 102.6),
            40.0,
        )
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_trashbag1",
            "Trash bags",
            "trashbag",
            on_pad(88.9, 101.9),
            140.0,
        )
        .decor(),
    );
    s.prop(Prop::new(
        "e_handtruck",
        "Hand truck",
        "hand_truck",
        on_pad(101.0, 103.1),
        200.0,
    ));
    s.prop(
        Prop::new(
            "e_aircon",
            "Air conditioner",
            "aircon",
            Vec3::new(106.3, 2.4, 110.0),
            90.0,
        )
        .at_origin()
        .decor(),
    );
    s.prop(Prop::new(
        "e_utility0",
        "Utility box",
        "utility_box_wide",
        on_pad(106.6, 113.0),
        270.0,
    ));
    s.prop(Prop::new(
        "e_utility1",
        "Utility box",
        "utility_box",
        on_pad(106.6, 114.3),
        270.0,
    ));
    s.prop(Prop::new(
        "e_ladder",
        "Ladder",
        "ladder",
        on_pad(85.2, 103.2),
        180.0,
    ));

    // Container yard (east).
    for (i, (name, x, y, z, yaw)) in [
        ("container_green", 116.0, 0.0, 84.0, 90.0),
        ("container_rust", 118.8, 0.0, 84.6, 92.0),
        ("container_blue", 117.4, 2.6, 84.2, 89.0),
        ("container_rust", 115.5, 0.0, 96.0, 4.0),
        ("container_green", 76.0, 0.0, 120.0, 18.0),
    ]
    .into_iter()
    .enumerate()
    {
        let base = if y > 0.0 { pad_top + y } else { pad_top };
        s.prop(
            Prop::new(
                &format!("e_container{i}"),
                "Shipping container",
                name,
                Vec3::new(x, base, z),
                yaw,
            )
            .at_origin(),
        );
    }
    s.prop(Prop::new(
        "e_generator",
        "Generator",
        "generator",
        on_pad(112.4, 90.6),
        250.0,
    ));
    s.prop(Prop::new(
        "e_compressor",
        "Compressor",
        "compressor",
        on_pad(112.0, 93.6),
        180.0,
    ));
    for (i, (name, x, z, yaw)) in [
        ("barrel_blue", 113.0, 81.0, 0.0),
        ("barrel_rust", 113.7, 81.5, 30.0),
        ("barrel_red", 113.2, 82.2, 70.0),
        ("barrel_blue", 119.5, 89.6, 10.0),
        ("propane", 112.6, 87.2, 0.0),
        ("propane", 112.9, 87.6, 50.0),
        ("lpg", 113.4, 87.1, 0.0),
        ("jerrycan", 111.9, 89.5, 95.0),
        ("jerrycan_green", 112.1, 90.0, 80.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(Prop::new(
            &format!("e_yard{i}"),
            "Supplies",
            name,
            on_pad(x, z),
            yaw,
        ));
    }

    // Campfire circle (west of the pad).
    let fire = on_pad(84.0, 91.5);
    s.prop(Prop::new("e_firepit", "Fire pit", "fire_pit", fire, 0.0).decor());
    s.prop(
        Prop::new(
            "e_embers",
            "Embers",
            "embers",
            fire + Vec3::new(0.0, 0.05, 0.0),
            30.0,
        )
        .at_origin()
        .decor()
        .renderer(json!({"emissive": [3200, 3200, 3200]})),
    );
    s.entity(
        "e_fire",
        "Campfire",
        json!({"Transform": {"position": [fire.x, fire.y + 0.05, fire.z]}, "VisualEffect": {"effect": "campfire"},
            "AudioSource": {"clip": format!("@{}#as_fire", ctx.lock.get("oga_fireplace").expect("fire").relative("fire.wav")), "looping": true, "referenceDistance": 2, "maxDistance": 30, "gain": 0.5},
            "AcousticSource": {}}),
    );
    s.commands.extend(light(
        "e_firelight",
        "Campfire light",
        fire + Vec3::new(0.0, 0.7, 0.0),
        json!({"color": [1.0, 0.5, 0.2, 1], "intensity": 3200, "range": 12}),
        None,
    ));
    for (i, (name, dx, dz, yaw)) in [
        ("crate_military", -1.9, 0.6, 75.0),
        ("crate_long", 1.6, -1.3, 30.0),
        ("barrel_stove", 1.8, 1.4, 0.0),
        ("ammo_crate", -0.6, -2.0, 160.0),
        ("crate_flat", -2.4, -1.0, 110.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(Prop::new(
            &format!("e_seat{i}"),
            "Seat",
            name,
            fire + Vec3::new(dx, 0.0, dz),
            yaw,
        ));
    }
    s.prop(
        Prop::new(
            "e_radio",
            "Radio",
            "radio",
            fire + Vec3::new(-1.9, 0.47, 0.62),
            160.0,
        )
        .decor(),
    );
    s.prop(
        Prop::new(
            "e_gasmask",
            "Gas mask",
            "gas_mask",
            fire + Vec3::new(1.55, 0.46, -1.15),
            40.0,
        )
        .decor(),
    );

    // Stores by the hangar door.
    for (i, (name, x, z, yaw)) in [
        ("crate_military", 90.0, 99.0, 5.0),
        ("crate_military", 90.1, 99.0, 0.0),
        ("crate_long", 91.9, 98.4, 90.0),
        ("crate_plastic", 90.6, 100.2, 20.0),
        ("crate_plastic", 91.2, 100.3, 0.0),
        ("box", 92.6, 99.6, 35.0),
        ("box", 92.9, 100.3, 80.0),
        ("ammo_crate", 87.8, 97.6, 100.0),
    ]
    .into_iter()
    .enumerate()
    {
        let lift = if i == 1 { 0.42 } else { 0.0 };
        s.prop(Prop::new(
            &format!("e_store{i}"),
            "Stores",
            name,
            on_pad(x, z) + Vec3::Y * lift,
            yaw,
        ));
    }

    // Radio mast (north-east, outside the hangar).
    s.prop(
        Prop::new("e_mast", "Radio mast", "mast", g(121.0, 124.0), 15.0)
            .at_origin()
            .renderer(json!({"emissive": [800, 800, 800]})),
    );
    s.commands.extend(light(
        "e_beacon",
        "Mast beacon",
        g(121.0, 124.0) + Vec3::new(0.0, 24.3, 0.0),
        json!({"color": [1.0, 0.1, 0.05, 1], "intensity": 400, "range": 12}),
        None,
    ));

    // Floodlights on the pad corners, aimed into the compound.
    for (i, (x, z, yaw)) in [
        (73.0, 79.0, 225.0),
        (119.0, 79.0, 135.0),
        (73.0, 105.0, 315.0),
    ]
    .into_iter()
    .enumerate()
    {
        let base = on_pad(x, z);
        s.prop(
            Prop::new(
                &format!("e_flood{i}"),
                "Floodlight",
                "floodlight",
                base,
                yaw,
            )
            .renderer(json!({"emissive": [4000, 3800, 3400]})),
        );
        let head = base + Vec3::new(0.0, 3.7, 0.0);
        let target = Vec3::new(C.x, 0.0, C.y);
        s.commands.extend(light(
            &format!("e_floodl{i}"),
            "Floodlight beam",
            head,
            json!({"kind": "spot", "color": [1.0, 0.9, 0.78, 1], "intensity": 14000, "range": 34, "innerAngle": 28, "outerAngle": 48}),
            Some(target - head),
        ));
    }
    // Security light above the hangar door, washing the apron.
    let door_light = Vec3::new(HANGAR.x - 6.0, 6.6, HANGAR.z - 13.4);
    s.prop(
        Prop::new(
            "e_seclight",
            "Security light",
            "security_light",
            door_light,
            180.0,
        )
        .at_origin()
        .decor()
        .renderer(json!({"emissive": [3000, 2900, 2700]})),
    );
    s.commands.extend(light(
        "e_seclightl",
        "Security light beam",
        door_light + Vec3::new(0.0, -0.2, -0.2),
        json!({"kind": "spot", "color": [0.92, 0.95, 1.0, 1], "intensity": 9000, "range": 22, "innerAngle": 30, "outerAngle": 55}),
        Some(Vec3::new(0.0, -1.0, -0.9)),
    ));
    // Warm work light spilling out of the half-open hangar door.
    s.commands.extend(light(
        "e_hangarglow",
        "Hangar interior glow",
        Vec3::new(HANGAR.x + 3.0, 4.5, HANGAR.z - 8.0),
        json!({"color": [1.0, 0.78, 0.5, 1], "intensity": 6000, "range": 18}),
        None,
    ));

    // Electricity poles along the road, with sagging wires between them.
    let poles = [(100.6, 64.0), (103.5, 46.0), (108.0, 30.0), (110.2, 14.0)];
    for (i, &(x, z)) in poles.iter().enumerate() {
        s.prop(Prop::new(&format!("e_pole{i}"), "Utility pole", "pole", g(x, z), 8.0).at_origin());
    }

    // Landscape: boulders, quiver trees, dead wood outside the fence.
    for (i, (name, x, z, yaw, scale)) in [
        ("boulder_big", 58.0, 84.0, 30.0, 1.4),
        ("boulder", 54.0, 80.0, 110.0, 1.0),
        ("boulder_flat", 61.0, 77.0, 200.0, 1.2),
        ("boulder_big", 138.0, 112.0, 60.0, 1.8),
        ("boulder", 141.0, 106.0, 10.0, 1.2),
        ("boulder_small", 136.0, 104.5, 80.0, 1.3),
        ("boulder", 84.0, 54.0, 250.0, 1.5),
        ("boulder_flat", 88.0, 50.0, 300.0, 1.0),
        ("boulder_big", 120.0, 56.0, 140.0, 1.3),
        ("boulder_small", 112.0, 58.0, 20.0, 1.6),
        ("boulder", 70.0, 146.0, 30.0, 2.0),
        ("boulder_big", 150.0, 70.0, 300.0, 2.2),
    ]
    .into_iter()
    .enumerate()
    {
        let p = Vec3::new(x, ground.lowest(x, z, 1.2 * scale) - 0.08 * scale, z);
        s.prop(Prop::new(&format!("e_boulder{i}"), "Boulder", name, p, yaw).scaled(scale));
    }
    for (i, (name, x, z, yaw, scale)) in [
        ("quiver_tree", 60.0, 90.0, 20.0, 2.6),
        ("quiver_tree_young", 55.5, 95.0, 140.0, 3.0),
        ("quiver_tree", 134.0, 98.0, 200.0, 2.9),
        ("quiver_tree", 82.0, 58.0, 80.0, 2.4),
        ("quiver_tree_young", 123.0, 62.0, 10.0, 2.8),
        ("quiver_tree", 145.0, 128.0, 300.0, 3.2),
        ("quiver_tree", 48.0, 128.0, 50.0, 3.0),
        ("quiver_tree_young", 112.0, 145.0, 240.0, 3.4),
        ("dead_quiver", 66.0, 70.0, 0.0, 2.2),
        ("dead_quiver", 131.0, 76.0, 90.0, 2.0),
    ]
    .into_iter()
    .enumerate()
    {
        let p = Vec3::new(x, ground.lowest(x, z, 0.3) - 0.05, z);
        s.prop(Prop::new(&format!("e_tree{i}"), "Quiver tree", name, p, yaw).scaled(scale));
    }
    for (i, (x, z, yaw)) in [
        (62.0, 100.0, 70.0),
        (130.0, 66.0, 160.0),
        (90.0, 46.0, 20.0),
    ]
    .into_iter()
    .enumerate()
    {
        s.prop(
            Prop::new(
                &format!("e_log{i}"),
                "Dead log",
                "log",
                Vec3::new(x, ground.lowest(x, z, 1.5), z),
                yaw,
            )
            .decor(),
        );
    }

    // Ambient sound and music.
    s.entity(
        "e_wind",
        "Wind",
        json!({"Transform": {"position": [C.x, 2.0, C.y]}, "AudioSource": {"clip": format!("@{}#as_wind", ctx.lock.get("oga_wind").expect("wind").relative("wind_loop.ogg")), "looping": true, "referenceDistance": 60, "maxDistance": 400, "gain": 0.2, "spatial": false}}),
    );
    s.entity(
        "e_music",
        "Music",
        json!({"MusicSource": {"music": reference(MUSIC)}}),
    );

    let calls = std::mem::take(&mut s.commands);
    ctx.apply(
        SCENE.0,
        "Outpost at dusk: compound, lights, campfire, player",
        calls,
    )
}

/// Static walls of a hangar shell centred at `origin` (boxes; the door opening stays
/// walkable).
pub fn hangar_colliders_at(s: &mut Scene<'_>, origin: Vec3) {
    let h = Hangar::OUTPOST;
    let (w, d) = (h.width, h.depth);
    let side = (w - h.door) / 2.0;
    for (i, (x, z, hx, hz)) in [
        (-w / 2.0, 0.0, 0.1, d / 2.0),
        (w / 2.0, 0.0, 0.1, d / 2.0),
        (0.0, d / 2.0, w / 2.0, 0.1),
        (-(h.door / 2.0 + side / 2.0), -d / 2.0, side / 2.0, 0.1),
        (h.door / 2.0 + side / 2.0, -d / 2.0, side / 2.0, 0.1),
        (-h.door / 4.0, -d / 2.0 - 0.25, h.door / 4.0, 0.1),
    ]
    .into_iter()
    .enumerate()
    {
        s.entity(
            &format!("e_hangarwall{i}"),
            "Hangar wall collider",
            json!({"Transform": {"position": [origin.x + x, origin.y + h.height / 2.0, origin.z + z]}, "RigidBody": {}, "Collider": {"halfExtents": [hx, h.height / 2.0, hz]}}),
        );
    }
}

/// Chain-link perimeter: 2 m sections along a rectangle, a gate, a few panels missing.
fn fence(s: &mut Scene<'_>, ground: &Ground) {
    let (x0, x1, z0, z1) = (64.0f32, 128.0f32, 72.0f32, 134.0f32);
    let mut n = 0;
    let mut run = |s: &mut Scene<'_>, a: Vec2, b: Vec2, skip: &dyn Fn(usize) -> bool| {
        let dir = (b - a).normalize();
        let yaw = (-dir.y).atan2(dir.x).to_degrees();
        let count = (a.distance(b) / 2.0).round() as usize;
        for i in 0..count {
            if skip(i) {
                continue;
            }
            let p = a + dir * (i as f32 * 2.0);
            let pos = Vec3::new(p.x, ground.lowest(p.x, p.y, 1.0) - 0.05, p.y);
            s.prop(
                Prop::new(&format!("e_fence{n}"), "Fence", "fence_section", pos, yaw)
                    .at_origin()
                    .decor(),
            );
            n += 1;
        }
    };
    // South side with the gate gap (x 92..100), then east, north, west.
    run(s, Vec2::new(x0, z0), Vec2::new(92.0, z0), &|_| false);
    run(s, Vec2::new(100.0, z0), Vec2::new(x1, z0), &|_| false);
    run(s, Vec2::new(x1, z0), Vec2::new(x1, z1), &|i| {
        i == 9 || i == 10
    });
    run(s, Vec2::new(x1, z1), Vec2::new(x0, z1), &|i| i == 22);
    run(s, Vec2::new(x0, z1), Vec2::new(x0, z0), &|i| {
        (14..17).contains(&i)
    });
    // The gate: a closing post, then two leaves swung open into the compound.
    for (i, (x, yaw)) in [(92.0f32, -70.0f32), (100.0, -110.0)]
        .into_iter()
        .enumerate()
    {
        s.prop(
            Prop::new(
                &format!("e_gate{i}"),
                "Gate",
                "fence_gate",
                Vec3::new(x, ground.height(x, z0) - 0.05, z0),
                yaw,
            )
            .at_origin()
            .decor(),
        );
    }
}
