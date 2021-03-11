use crate::{visibility::Light, ExternalEvent};
use entity_table::{Entity, EntityAllocator};
use grid_2d::{Coord, Size};
use rand::Rng;
use rgb24::Rgb24;
use serde::{Deserialize, Serialize};

mod air;
pub use air::Air;

mod spatial;
use spatial::SpatialTable;

pub mod player;

mod data;
pub use data::{
    Armour, Disposition, EntityData, HitPoints, Item, Layer, Location, MeleeWeapon, NpcAction,
    Oxygen, RangedWeapon, Tile,
};
use data::{Components, Npc};

mod realtime_periodic;
pub use realtime_periodic::animation::{
    Context as AnimationContext, FRAME_DURATION as ANIMATION_FRAME_DURATION,
};
use realtime_periodic::data::RealtimeComponents;

mod query;

mod explosion;
pub use explosion::spec as explosion_spec;

mod action;
pub use action::Error as ActionError;

mod spawn;
pub use spawn::make_player;

#[derive(Debug, Serialize, Deserialize)]
pub struct World {
    pub level: u32,
    pub entity_allocator: EntityAllocator,
    pub components: Components,
    pub realtime_components: RealtimeComponents,
    pub spatial_table: SpatialTable,
    pub air: Air,
}

impl World {
    pub fn new(size: Size, level: u32) -> Self {
        let entity_allocator = EntityAllocator::default();
        let components = Components::default();
        let realtime_components = RealtimeComponents::default();
        let spatial_table = SpatialTable::new(size);
        let air = Air::new(size);
        Self {
            entity_allocator,
            components,
            realtime_components,
            spatial_table,
            level,
            air,
        }
    }
}

impl World {
    pub fn to_render_entity(&self, entity: Entity) -> Option<ToRenderEntity> {
        let tile_component = &self.components.tile;
        let spatial_table = &self.spatial_table;
        let realtime_fade_component = &self.realtime_components.fade;
        let colour_hint_component = &self.components.colour_hint;
        let blood_component = &self.components.blood;
        let ignore_lighting_component = &self.components.ignore_lighting;
        let hit_points = &self.components.hit_points;
        let armour = &self.components.armour;
        let next_action = &self.components.next_action;
        let skeleton_respawn = &self.components.skeleton_respawn;
        let fade = realtime_fade_component
            .get(entity)
            .and_then(|f| f.state.fading());
        let colour_hint = colour_hint_component.get(entity).cloned();
        let blood = blood_component.contains(entity);
        let ignore_lighting = ignore_lighting_component.contains(entity);
        let hit_points = hit_points.get(entity).cloned();
        let armour = armour.get(entity).cloned();
        let next_action = next_action.get(entity).cloned();
        let tile = tile_component.get(entity).cloned()?;
        let skeleton_respawn = skeleton_respawn.get(entity).cloned();
        if let Some(location) = spatial_table.location_of(entity) {
            Some(ToRenderEntity {
                coord: location.coord,
                layer: location.layer,
                tile,
                fade,
                colour_hint,
                blood,
                ignore_lighting,
                hit_points,
                armour,
                next_action,
                skeleton_respawn,
            })
        } else {
            None
        }
    }
    pub fn to_render_entities<'a>(&'a self) -> impl 'a + Iterator<Item = ToRenderEntity> {
        let tile_component = &self.components.tile;
        let spatial_table = &self.spatial_table;
        let realtime_fade_component = &self.realtime_components.fade;
        let colour_hint_component = &self.components.colour_hint;
        let blood_component = &self.components.blood;
        let ignore_lighting_component = &self.components.ignore_lighting;
        let hit_points = &self.components.hit_points;
        let armour = &self.components.armour;
        let next_action = &self.components.next_action;
        let skeleton_respawn = &self.components.skeleton_respawn;
        tile_component.iter().filter_map(move |(entity, &tile)| {
            if let Some(location) = spatial_table.location_of(entity) {
                let fade = realtime_fade_component
                    .get(entity)
                    .and_then(|f| f.state.fading());
                let colour_hint = colour_hint_component.get(entity).cloned();
                let blood = blood_component.contains(entity);
                let ignore_lighting = ignore_lighting_component.contains(entity);
                let hit_points = hit_points.get(entity).cloned();
                let armour = armour.get(entity).cloned();
                let next_action = next_action.get(entity).cloned();
                let skeleton_respawn = skeleton_respawn.get(entity).cloned();
                Some(ToRenderEntity {
                    coord: location.coord,
                    layer: location.layer,
                    tile,
                    fade,
                    colour_hint,
                    blood,
                    ignore_lighting,
                    hit_points,
                    armour,
                    next_action,
                    skeleton_respawn,
                })
            } else {
                None
            }
        })
    }

    pub fn to_render_entities_realtime<'a>(
        &'a self,
    ) -> impl 'a + Iterator<Item = ToRenderEntityRealtime> {
        let tile_component = &self.components.tile;
        let spatial_table = &self.spatial_table;
        let realtime_fade_component = &self.realtime_components.fade;
        let colour_hint_component = &self.components.colour_hint;
        let particle_component = &self.components.particle;
        let realtime_component = &self.components.realtime;
        realtime_component.iter().filter_map(move |(entity, &())| {
            if let Some(location) = spatial_table.location_of(entity) {
                let fade = realtime_fade_component
                    .get(entity)
                    .and_then(|f| f.state.fading());
                let tile = tile_component.get(entity).cloned();
                let colour_hint = colour_hint_component.get(entity).cloned();
                let particle = particle_component.contains(entity);
                Some(ToRenderEntityRealtime {
                    coord: location.coord,
                    layer: location.layer,
                    tile,
                    fade,
                    colour_hint,
                    particle,
                })
            } else {
                None
            }
        })
    }

    pub fn all_lights_by_coord<'a>(&'a self) -> impl 'a + Iterator<Item = (Coord, &'a Light)> {
        self.components
            .light
            .iter()
            .filter_map(move |(entity, light)| {
                self.spatial_table
                    .coord_of(entity)
                    .map(|coord| (coord, light))
            })
    }

    pub fn character_info(&self, entity: Entity) -> Option<CharacterInfo> {
        let coord = self.spatial_table.coord_of(entity)?;
        let &hit_points = self.components.hit_points.get(entity)?;
        let &oxygen = self.components.oxygen.get(entity)?;
        Some(CharacterInfo {
            coord,
            hit_points,
            oxygen,
        })
    }

    pub fn cleanup(&mut self) -> Option<PlayerDied> {
        let mut ret = None;
        for (entity, hp) in self.components.hit_points.iter() {
            if hp.current == 0 {
                self.components.to_remove.insert(entity, ());
            }
        }
        for entity in self.components.to_remove.entities().collect::<Vec<_>>() {
            if self.components.player.contains(entity) {
                let player_data = self.components.remove_entity_data(entity);
                ret = Some(PlayerDied(player_data));
            } else {
                self.components.remove_entity(entity);
            }
            self.spatial_table.remove(entity);
            self.entity_allocator.free(entity);
        }
        ret
    }
}

pub struct PlayerDied(pub EntityData);

impl World {
    pub fn entity_coord(&self, entity: Entity) -> Option<Coord> {
        self.spatial_table.coord_of(entity)
    }
    pub fn entity_player(&self, entity: Entity) -> Option<&player::Player> {
        self.components.player.get(entity)
    }
    pub fn entity_npc(&self, entity: Entity) -> &Npc {
        self.components.npc.get(entity).unwrap()
    }
    pub fn entity_exists(&self, entity: Entity) -> bool {
        self.entity_allocator.exists(entity) && !self.components.to_remove.contains(entity)
    }
    pub fn size(&self) -> Size {
        self.spatial_table.grid_size()
    }
    pub fn is_gameplay_blocked(&self) -> bool {
        !self.components.blocks_gameplay.is_empty()
    }
    pub fn animation_tick<R: Rng>(
        &mut self,
        animation_context: &mut AnimationContext,
        external_events: &mut Vec<ExternalEvent>,
        rng: &mut R,
    ) {
        animation_context.tick(self, external_events, rng)
    }
    pub fn commit_to_next_action(&mut self, entity: Entity, next_action: NpcAction) {
        self.components.next_action.insert(entity, next_action);
    }
    pub fn next_npc_action(&self, entity: Entity) -> Option<NpcAction> {
        self.components.next_action.get(entity).cloned()
    }
    pub fn clone_entity_data(&self, entity: Entity) -> EntityData {
        self.components.clone_entity_data(entity)
    }
}

pub struct ToRenderEntityRealtime {
    pub coord: Coord,
    pub layer: Option<Layer>,
    pub tile: Option<Tile>,
    pub fade: Option<u8>,
    pub colour_hint: Option<Rgb24>,
    pub particle: bool,
}

pub struct ToRenderEntity {
    pub coord: Coord,
    pub layer: Option<Layer>,
    pub tile: Tile,
    pub fade: Option<u8>,
    pub colour_hint: Option<Rgb24>,
    pub blood: bool,
    pub ignore_lighting: bool,
    pub hit_points: Option<HitPoints>,
    pub armour: Option<Armour>,
    pub next_action: Option<NpcAction>,
    pub skeleton_respawn: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct CharacterInfo {
    pub coord: Coord,
    pub hit_points: HitPoints,
    pub oxygen: Oxygen,
}
