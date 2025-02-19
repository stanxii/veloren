use crate::{column::ColumnSample, sim::SimChunk, IndexRef, CONFIG};
use common::{
    assets::{self, AssetExt},
    generation::{ChunkSupplement, EntityInfo},
    resources::TimeOfDay,
    terrain::Block,
    time::DayPeriod,
    vol::{BaseVol, ReadVol, RectSizedVol, WriteVol},
};
use rand::prelude::*;
use serde::Deserialize;
use std::f32;
use vek::*;

type Weight = u32;
type Min = u8;
type Max = u8;

fn close(x: f32, tgt: f32, falloff: f32) -> f32 {
    (1.0 - (x - tgt).abs() / falloff).max(0.0).powf(0.125)
}

const BASE_DENSITY: f32 = 1.0e-5; // Base wildlife density

#[derive(Clone, Debug, Deserialize)]
pub struct SpawnEntry {
    /// User-facing info for wiki, statistical tools, etc.
    pub name: String,
    pub note: String,
    /// Rules describing what and when to spawn
    pub rules: Vec<Pack>,
}

impl assets::Asset for SpawnEntry {
    type Loader = assets::RonLoader;

    const EXTENSION: &'static str = "ron";
}

impl SpawnEntry {
    pub fn from(asset_specifier: &str) -> Self { Self::load_expect(asset_specifier).read().clone() }

    pub fn request(&self, requested_period: DayPeriod, underwater: bool) -> Option<Pack> {
        self.rules
            .iter()
            .find(|pack| {
                let time_match = pack
                    .day_period
                    .iter()
                    .any(|period| *period == requested_period);
                let water_match = pack.is_underwater == underwater;
                time_match && water_match
            })
            .cloned()
    }
}

/// Dataset of animals to spawn
///
/// Example:
/// ```text
///        Pack(
///            groups: [
///                (3, (1, 2, "common.entity.wild.aggressive.frostfang")),
///                (1, (1, 1, "common.entity.wild.aggressive.snow_leopard")),
///                (1, (1, 1, "common.entity.wild.aggressive.yale")),
///                (1, (1, 1, "common.entity.wild.aggressive.grolgar")),
///            ],
///            is_underwater: false,
///            day_period: [Night, Morning, Noon, Evening],
///        ),
/// ```
/// Groups:
/// ```text
///                (3, (1, 2, "common.entity.wild.aggressive.frostfang")),
/// ```
/// (3, ...) means that it has x3 chance to spawn (3/6 when every other has
/// 1/6).
///
/// (.., (1, 2, ...)) is `1..=2` group size which means that it has
/// chance to spawn as single mob or in pair
///
/// (..., (..., "common.entity.wild.aggressive.frostfang")) corresponds
/// to `assets/common/entity/wild/aggressive/frostfang.ron` file with
/// EntityConfig
///
/// Underwater:
/// `is_underwater: false` means mobs from this pack can't be spawned underwater
/// in rivers, lakes or ocean
///
/// Day period:
/// `day_period: [Night, Morning, Noon, Evening]`
/// means that mobs from this pack may be spawned in any day period without
/// exception
#[derive(Clone, Debug, Deserialize)]
pub struct Pack {
    pub groups: Vec<(Weight, (Min, Max, String))>,
    pub is_underwater: bool,
    pub day_period: Vec<DayPeriod>,
}

impl Pack {
    pub fn generate(&self, pos: Vec3<f32>, dynamic_rng: &mut impl Rng) -> (EntityInfo, u8) {
        let (_, (from, to, entity_asset)) = self
            .groups
            .choose_weighted(dynamic_rng, |(p, _group)| *p)
            .expect("Failed to choose group");
        let entity = EntityInfo::at(pos).with_asset_expect(entity_asset);
        let group_size = dynamic_rng.gen_range(*from..=*to);

        (entity, group_size)
    }
}

pub type DensityFn = fn(&SimChunk, &ColumnSample) -> f32;

pub fn spawn_manifest() -> Vec<(&'static str, DensityFn)> {
    // NOTE: Order matters.
    // Entries with more specific requirements
    // and overall scarcity should come first, where possible.
    vec![
        // **Tundra**
        // Rock animals
        ("world.wildlife.spawn.tundra.rock", |c, col| {
            close(c.temp, CONFIG.snow_temp, 0.15) * BASE_DENSITY * col.rock * 1.0
        }),
        // Core animals
        ("world.wildlife.spawn.tundra.core", |c, _col| {
            close(c.temp, CONFIG.snow_temp, 0.15) * BASE_DENSITY * 0.5
        }),
        // Snowy animals
        ("world.wildlife.spawn.tundra.snow", |c, col| {
            close(c.temp, CONFIG.snow_temp, 0.3) * BASE_DENSITY * col.snow_cover as i32 as f32 * 1.0
        }),
        // Forest animals
        ("world.wildlife.spawn.tundra.forest", |c, col| {
            close(c.temp, CONFIG.snow_temp, 0.3) * col.tree_density * BASE_DENSITY * 1.4
        }),
        // **Taiga**
        // Forest core animals
        ("world.wildlife.spawn.taiga.core_forest", |c, col| {
            close(c.temp, CONFIG.snow_temp + 0.2, 0.2) * col.tree_density * BASE_DENSITY * 0.4
        }),
        // Core animals
        ("world.wildlife.spawn.taiga.core", |c, _col| {
            close(c.temp, CONFIG.snow_temp + 0.2, 0.2) * BASE_DENSITY * 1.0
        }),
        // Forest area animals
        ("world.wildlife.spawn.taiga.forest", |c, col| {
            close(c.temp, CONFIG.snow_temp + 0.2, 0.6) * col.tree_density * BASE_DENSITY * 0.9
        }),
        // Area animals
        ("world.wildlife.spawn.taiga.area", |c, _col| {
            close(c.temp, CONFIG.snow_temp + 0.2, 0.6) * BASE_DENSITY * 5.0
        }),
        // Water animals
        ("world.wildlife.spawn.taiga.water", |c, col| {
            close(c.temp, CONFIG.snow_temp, 0.15) * col.tree_density * BASE_DENSITY * 5.0
        }),
        // **Temperate**
        // Area rare
        ("world.wildlife.spawn.temperate.rare", |c, _col| {
            close(c.temp, CONFIG.temperate_temp, 0.8) * BASE_DENSITY * 0.08
        }),
        // River wildlife
        ("world.wildlife.spawn.temperate.river", |_c, col| {
            close(col.temp, CONFIG.temperate_temp, 0.6)
                * if col.water_dist.map(|d| d < 10.0).unwrap_or(false) {
                    0.001
                } else {
                    0.0
                }
        }),
        // Forest animals
        ("world.wildlife.spawn.temperate.wood", |c, col| {
            close(c.temp, CONFIG.temperate_temp + 0.1, 0.5) * col.tree_density * BASE_DENSITY * 1.0
        }),
        // Rainforest animals
        ("world.wildlife.spawn.temperate.rainforest", |c, _col| {
            close(c.temp, CONFIG.temperate_temp + 0.1, 0.6)
                * close(c.humidity, CONFIG.forest_hum, 0.6)
                * BASE_DENSITY
                * 4.0
        }),
        // Water animals
        ("world.wildlife.spawn.temperate.water", |c, col| {
            close(c.temp, CONFIG.temperate_temp, 1.0) * col.tree_density * BASE_DENSITY * 5.0
        }),
        // **Jungle**
        // Rainforest animals
        ("world.wildlife.spawn.jungle.rainforest", |c, _col| {
            close(c.temp, CONFIG.tropical_temp + 0.2, 0.2)
                * close(c.humidity, CONFIG.jungle_hum, 0.2)
                * BASE_DENSITY
                * 2.8
        }),
        // Rainforest area animals
        ("world.wildlife.spawn.jungle.rainforest_area", |c, _col| {
            close(c.temp, CONFIG.tropical_temp + 0.2, 0.3)
                * close(c.humidity, CONFIG.jungle_hum, 0.2)
                * BASE_DENSITY
                * 8.0
        }),
        // **Tropical**
        // Rare river animals
        ("world.wildlife.spawn.tropical.river_rare", |_c, col| {
            close(col.temp, CONFIG.tropical_temp + 0.2, 0.5)
                * if col.water_dist.map(|d| d < 10.0).unwrap_or(false) {
                    0.0001
                } else {
                    0.0
                }
        }),
        // River animals
        ("world.wildlife.spawn.tropical.river", |_c, col| {
            close(col.temp, CONFIG.tropical_temp, 0.5)
                * if col.water_dist.map(|d| d < 10.0).unwrap_or(false) {
                    0.001
                } else {
                    0.0
                }
        }),
        // Rainforest area animals
        ("world.wildlife.spawn.tropical.rainforest", |c, _col| {
            close(c.temp, CONFIG.tropical_temp + 0.1, 0.4)
                * close(c.humidity, CONFIG.desert_hum, 0.4)
                * BASE_DENSITY
                * 2.0
        }),
        // Rock animals
        ("world.wildlife.spawn.tropical.rock", |c, col| {
            close(c.temp, CONFIG.tropical_temp + 0.1, 0.5) * col.rock * BASE_DENSITY * 5.0
        }),
        // **Desert**
        // Area animals
        ("world.wildlife.spawn.desert.area", |c, _col| {
            close(c.temp, CONFIG.tropical_temp + 0.1, 0.4)
                * close(c.humidity, CONFIG.desert_hum, 0.4)
                * BASE_DENSITY
                * 0.8
        }),
        // Wasteland animals
        ("world.wildlife.spawn.desert.wasteland", |c, _col| {
            close(c.temp, CONFIG.desert_temp + 0.2, 0.3)
                * close(c.humidity, CONFIG.desert_hum, 0.5)
                * BASE_DENSITY
                * 1.3
        }),
        // River animals
        ("world.wildlife.spawn.desert.river", |_c, col| {
            close(col.temp, CONFIG.desert_temp + 0.2, 0.3)
                * if col.water_dist.map(|d| d < 10.0).unwrap_or(false) {
                    0.0001
                } else {
                    0.0
                }
        }),
        // Hot area desert
        ("world.wildlife.spawn.desert.hot", |c, _col| {
            close(c.temp, CONFIG.desert_temp + 0.2, 0.3) * BASE_DENSITY * 3.8
        }),
    ]
}

pub fn apply_wildlife_supplement<'a, R: Rng>(
    // NOTE: Used only for dynamic elements like chests and entities!
    dynamic_rng: &mut R,
    wpos2d: Vec2<i32>,
    mut get_column: impl FnMut(Vec2<i32>) -> Option<&'a ColumnSample<'a>>,
    vol: &(impl BaseVol<Vox = Block> + RectSizedVol + ReadVol + WriteVol),
    index: IndexRef,
    chunk: &SimChunk,
    supplement: &mut ChunkSupplement,
    time: Option<TimeOfDay>,
) {
    let scatter = &index.wildlife_spawns;

    for y in 0..vol.size_xy().y as i32 {
        for x in 0..vol.size_xy().x as i32 {
            let offs = Vec2::new(x, y);

            let wpos2d = wpos2d + offs;

            // Sample terrain
            let col_sample = if let Some(col_sample) = get_column(offs) {
                col_sample
            } else {
                continue;
            };

            let underwater = col_sample.water_level > col_sample.alt;
            let current_day_period;
            if let Some(time) = time {
                current_day_period = DayPeriod::from(time.0)
            } else {
                current_day_period = DayPeriod::Noon
            }

            let entity_group = scatter
                .iter()
                .enumerate()
                .find_map(|(_i, (entry, get_density))| {
                    let density = get_density(chunk, col_sample);
                    (density > 0.0)
                        .then(|| {
                            entry
                                .read()
                                .request(current_day_period, underwater)
                                .and_then(|pack| {
                                    (dynamic_rng.gen::<f32>() < density * col_sample.spawn_rate
                                        && col_sample.gradient < Some(1.3))
                                    .then(|| pack)
                                })
                        })
                        .flatten()
                });

            let alt = col_sample.alt as i32;

            if let Some(pack) = entity_group {
                let (entity, group_size) = pack.generate(
                    (wpos2d.map(|e| e as f32) + 0.5).with_z(alt as f32),
                    dynamic_rng,
                );
                for e in 0..group_size {
                    // Choose a nearby position
                    let offs_wpos2d = (Vec2::new(
                        (e as f32 / group_size as f32 * 2.0 * f32::consts::PI).sin(),
                        (e as f32 / group_size as f32 * 2.0 * f32::consts::PI).cos(),
                    ) * (5.0 + dynamic_rng.gen::<f32>().powf(0.5) * 5.0))
                        .map(|e| e as i32);
                    // Clamp position to chunk
                    let offs_wpos2d = (offs + offs_wpos2d)
                        .clamped(Vec2::zero(), vol.size_xy().map(|e| e as i32) - 1)
                        - offs;

                    // Find the intersection between ground and air, if there is one near the
                    // surface
                    if let Some(solid_end) = (-8..8).find(|z| {
                        (0..2).all(|z2| {
                            vol.get(Vec3::new(offs.x, offs.y, alt) + offs_wpos2d.with_z(z + z2))
                                .map(|b| !b.is_solid())
                                .unwrap_or(true)
                        })
                    }) {
                        let mut entity = entity.clone();
                        entity.pos += offs_wpos2d.with_z(solid_end).map(|e| e as f32);
                        supplement.add_entity(entity);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashbrown::{HashMap, HashSet};

    // Checks that each entry in spawn manifest is loadable
    #[test]
    fn test_load_entries() {
        let scatter = spawn_manifest();
        for (entry, _) in scatter.into_iter() {
            std::mem::drop(SpawnEntry::from(entry));
        }
    }

    // Check that each spawn entry has unique name
    #[test]
    fn test_name_uniqueness() {
        let scatter = spawn_manifest();
        let mut names = HashMap::new();
        for (entry, _) in scatter.into_iter() {
            let SpawnEntry { name, .. } = SpawnEntry::from(entry);
            if let Some(old_entry) = names.insert(name, entry) {
                panic!("{}: Found name duplicate with {}", entry, old_entry);
            }
        }
    }

    // Checks that day_period aren't overlapping
    // Quite strict rule, but otherwise may produce unexpected behaviour
    #[test]
    fn test_check_day_periods() {
        let scatter = spawn_manifest();
        for (entry, _) in scatter.into_iter() {
            let mut day_periods = HashSet::with_capacity(4);
            let SpawnEntry { rules, .. } = SpawnEntry::from(entry);
            for pack in rules {
                let Pack {
                    day_period,
                    is_underwater,
                    ..
                } = pack;
                for period in day_period {
                    if !day_periods.insert((period, is_underwater)) {
                        panic!(
                            r#"
                        == {}: ==
                            Found rules with duplicated `day_period` and `is_underwater`
                        If you have two of such entries,
                        there are big chances that second rule will be unreachable.

                            If you want some animals spawned in different Packs
                        with same day_period, add those animals to both Packs
                        and choose day_period to not overlap.
"#,
                            entry
                        )
                    }
                }
            }
        }
    }

    // Checks that each entity is loadable
    #[test]
    fn test_load_entities() {
        let scatter = spawn_manifest();
        for (entry, _) in scatter.into_iter() {
            let SpawnEntry { rules, .. } = SpawnEntry::from(entry);
            for pack in rules {
                let Pack { groups, .. } = pack;
                for group in &groups {
                    println!("{}:", entry);
                    let (_, (_, _, asset)) = group;
                    let dummy_pos = Vec3::new(0.0, 0.0, 0.0);
                    let entity = EntityInfo::at(dummy_pos).with_asset_expect(asset);
                    std::mem::drop(entity);
                }
            }
        }
    }

    // Checks that group distribution has valid form
    #[test]
    fn test_group_choose() {
        let scatter = spawn_manifest();
        for (entry, _) in scatter.into_iter() {
            let SpawnEntry { rules, .. } = SpawnEntry::from(entry);
            for pack in rules {
                let Pack { groups, .. } = pack;
                let dynamic_rng = &mut rand::thread_rng();
                let _ = groups
                    .choose_weighted(dynamic_rng, |(p, _group)| *p)
                    .unwrap_or_else(|err| {
                        panic!("{}: Failed to choose random group. Err: {}", entry, err)
                    });
            }
        }
    }
}
