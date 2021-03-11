use crate::{
    world::{
        data::{DoorState, Item, OnCollision, ProjectileDamage, Tile},
        explosion, player,
        realtime_periodic::{core::ScheduledRealtimePeriodicState, movement},
        spatial::{Layer, Location, SpatialTable},
        ActionError, ExternalEvent, World,
    },
    VisibilityGrid,
};
use direction::{CardinalDirection, Direction};
use entity_table::Entity;
use grid_2d::Coord;
use rand::{seq::IteratorRandom, seq::SliceRandom, Rng};
use std::collections::{HashSet, VecDeque};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub enum Error {
    WalkIntoSolidCell,
    CannotAffordUpgrade,
}

impl World {
    pub fn wait<R: Rng>(&mut self, entity: Entity, rng: &mut R) {
        if let Some(coord) = self.spatial_table.coord_of(entity) {
            self.after_player_move(entity, coord, rng);
        }
    }
    fn after_player_move<R: Rng>(&mut self, character: Entity, target_coord: Coord, rng: &mut R) {}

    pub fn character_pull_in_direction<R: Rng>(
        &mut self,
        character: Entity,
        direction: CardinalDirection,
        rng: &mut R,
    ) {
        let current_coord = if let Some(coord) = self.spatial_table.coord_of(character) {
            coord
        } else {
            panic!("failed to find coord for {:?}", character);
        };
        let target_coord = current_coord + direction.coord();
        if let Some(&cell) = self.spatial_table.layers_at(target_coord) {
            if let Some(feature_entity) = cell.feature {
                if self.components.solid.contains(feature_entity) {
                    return;
                }
            }
        } else {
            return;
        }
        let _ = self
            .spatial_table
            .update_coord(character, target_coord)
            .map_err(|e| e.unwrap_occupied_by());
    }

    pub fn character_walk_in_direction<R: Rng>(
        &mut self,
        character: Entity,
        direction: CardinalDirection,
        rng: &mut R,
    ) -> Result<Option<crate::GameControlFlow>, Error> {
        if let Some(move_half_speed) = self.components.move_half_speed.get_mut(character) {
            if move_half_speed.skip_next_move {
                move_half_speed.skip_next_move = false;
                return Ok(None);
            }
            move_half_speed.skip_next_move = true;
        }
        let current_coord = if let Some(coord) = self.spatial_table.coord_of(character) {
            coord
        } else {
            panic!("failed to find coord for {:?}", character);
        };
        let target_coord = current_coord + direction.coord();
        if let Some(&cell) = self.spatial_table.layers_at(target_coord) {
            if let Some(feature_entity) = cell.feature {
                if self.components.solid.contains(feature_entity) {
                    if let Some(DoorState::Closed) =
                        self.components.door_state.get(feature_entity).cloned()
                    {
                        self.open_door(feature_entity);
                        return Ok(None);
                    }
                    return Err(Error::WalkIntoSolidCell);
                }
                if self.components.upgrade.contains(feature_entity) {
                    if self.components.player.contains(character) {
                        return Ok(Some(crate::GameControlFlow::Upgrade));
                    } else {
                        return Err(Error::WalkIntoSolidCell);
                    }
                }
            }
        } else {
            return Err(Error::WalkIntoSolidCell);
        }
        if let Err(occupant) = self
            .spatial_table
            .update_coord(character, target_coord)
            .map_err(|e| e.unwrap_occupied_by())
        {
            self.melee_attack(character, occupant, direction, rng);
        } else {
            if self.components.player.contains(character) {
                self.after_player_move(character, target_coord, rng);
            }
        }
        Ok(None)
    }

    fn player_melee_attack<R: Rng>(
        &mut self,
        attacker: Entity,
        victim: Entity,
        direction: CardinalDirection,
        rng: &mut R,
    ) {
        let player = self.components.player.get(attacker).unwrap();
        let pen = player.melee_pen();
        if pen
            >= self
                .components
                .armour
                .get(victim)
                .expect("npc lacks armour")
                .value
        {
            let mut dmg = player.melee_dmg();
            if player.traits.double_damage {
                dmg *= 2;
            }
            self.damage_character(victim, dmg, rng);
        }
        let player = self.components.player.get(attacker).unwrap();
        for ability in player.melee_weapon.abilities.clone() {
            use player::WeaponAbility;
            match ability {
                WeaponAbility::KnockBack => {
                    self.character_push_in_direction(victim, direction.direction());
                    self.character_push_in_direction(victim, direction.direction());
                }
                _ => (),
            }
        }
        self.wait(attacker, rng);
    }

    fn npc_melee_attack<R: Rng>(&mut self, attacker: Entity, victim: Entity, rng: &mut R) {
        let &damage = self
            .components
            .damage
            .get(attacker)
            .expect("npc lacks damage component");
        self.damage_character(victim, damage, rng);
    }

    fn melee_attack<R: Rng>(
        &mut self,
        attacker: Entity,
        victim: Entity,
        direction: CardinalDirection,
        rng: &mut R,
    ) {
        if self.components.player.get(attacker).is_some() {
            self.player_melee_attack(attacker, victim, direction, rng);
        } else if self.components.player.get(victim).is_some() {
            self.npc_melee_attack(attacker, victim, rng);
        }
    }

    fn open_door(&mut self, door: Entity) {
        self.components.solid.remove(door);
        self.components.opacity.remove(door);
        let axis = match self
            .components
            .tile
            .get(door)
            .expect("door lacks tile component")
        {
            Tile::DoorClosed(axis) | Tile::DoorOpen(axis) => *axis,
            _ => panic!("unexpecgted tile on door"),
        };
        self.components.tile.insert(door, Tile::DoorOpen(axis));
        self.components.door_close_countdown.insert(door, 4);
    }

    fn close_door(&mut self, door: Entity) {
        self.components.solid.insert(door, ());
        self.components.opacity.insert(door, 255);
        let axis = match self
            .components
            .tile
            .get(door)
            .expect("door lacks tile component")
        {
            Tile::DoorClosed(axis) | Tile::DoorOpen(axis) => *axis,
            _ => panic!("unexpecgted tile on door"),
        };
        self.components.tile.insert(door, Tile::DoorClosed(axis));
    }

    pub fn process_oxygen<R: Rng>(&mut self, entity: Entity, rng: &mut R) {
        if let Some(oxygen) = self.components.oxygen.get_mut(entity) {
            if let Some(coord) = self.spatial_table.coord_of(entity) {
                if self.air.has_air(coord) {
                    if oxygen.current < oxygen.max {
                        oxygen.current += 1;
                    }
                } else {
                    if oxygen.current == 0 {
                        self.damage_character(entity, 1, rng);
                    } else {
                        oxygen.current -= 1;
                    }
                }
            }
        }
    }

    pub fn process_door_close_countdown(&mut self) {
        let mut to_close = Vec::new();
        for (entity, door_close_countdown) in self.components.door_close_countdown.iter_mut() {
            if let Some(coord) = self.spatial_table.coord_of(entity) {
                if let Some(layers) = self.spatial_table.layers_at(coord) {
                    if layers.character.is_some() {
                        *door_close_countdown = 4;
                        continue;
                    }
                }
                if *door_close_countdown == 0 {
                    to_close.push(entity);
                } else {
                    *door_close_countdown -= 1;
                }
            }
        }
        for entity in to_close {
            self.components.door_close_countdown.remove(entity);
            self.close_door(entity);
        }
    }

    pub fn character_fire_bullet(&mut self, character: Entity, target: Coord) {
        let character_coord = self.spatial_table.coord_of(character).unwrap();
        if character_coord == target {
            return;
        }
        self.spawn_bullet(character_coord, target);
        self.spawn_flash(character_coord);
    }

    fn blink<R: Rng>(&mut self, entity: Entity, coord: Coord, rng: &mut R) {
        self.spatial_table.update_coord(entity, coord).unwrap();
        if self.components.player.contains(entity) {
            self.after_player_move(entity, coord, rng);
        }
    }

    pub fn projectile_stop<R: Rng>(
        &mut self,
        projectile_entity: Entity,
        external_events: &mut Vec<ExternalEvent>,
        rng: &mut R,
    ) {
        if let Some(current_coord) = self.spatial_table.coord_of(projectile_entity) {
            if let Some(on_collision) = self.components.on_collision.get(projectile_entity).cloned()
            {
                match on_collision {
                    OnCollision::Explode(explosion_spec) => {
                        explosion::explode(
                            self,
                            current_coord,
                            explosion_spec,
                            external_events,
                            rng,
                        );
                        self.spatial_table.remove(projectile_entity);
                        self.components.remove_entity(projectile_entity);
                        self.entity_allocator.free(projectile_entity);
                        self.realtime_components.remove_entity(projectile_entity);
                    }
                    OnCollision::Remove => {
                        self.spatial_table.remove(projectile_entity);
                        self.components.remove_entity(projectile_entity);
                        self.entity_allocator.free(projectile_entity);
                        self.realtime_components.remove_entity(projectile_entity);
                    }
                    OnCollision::RemoveRealtime => {
                        self.realtime_components.remove_entity(projectile_entity);
                        self.components.realtime.remove(projectile_entity);
                        self.components.blocks_gameplay.remove(projectile_entity);
                    }
                }
            }
        }
        self.realtime_components.movement.remove(projectile_entity);
    }

    pub fn projectile_move<R: Rng>(
        &mut self,
        projectile_entity: Entity,
        movement_direction: Direction,
        external_events: &mut Vec<ExternalEvent>,
        rng: &mut R,
    ) {
        if let Some(current_coord) = self.spatial_table.coord_of(projectile_entity) {
            let next_coord = current_coord + movement_direction.coord();
            let collides_with = self
                .components
                .collides_with
                .get(projectile_entity)
                .cloned()
                .unwrap_or_default();
            if let Some(&spatial_cell) = self.spatial_table.layers_at(next_coord) {
                if let Some(character_entity) = spatial_cell.character {
                    if let Some(&projectile_damage) =
                        self.components.projectile_damage.get(projectile_entity)
                    {
                        self.apply_projectile_damage(
                            projectile_entity,
                            projectile_damage,
                            movement_direction,
                            character_entity,
                            rng,
                        );
                    }
                }
                if let Some(entity_in_cell) = spatial_cell.feature.or(spatial_cell.character) {
                    if (collides_with.solid && self.components.solid.contains(entity_in_cell))
                        || (collides_with.character
                            && self.components.character.contains(entity_in_cell))
                    {
                        if let Some(&projectile_damage) =
                            self.components.projectile_damage.get(projectile_entity)
                        {
                            if self.components.destructible.contains(entity_in_cell) {
                                let mut hull_pen_percent = projectile_damage.hull_pen_percent;
                                for (_, player) in self.components.player.iter() {
                                    if player.traits.reduce_hull_pen {
                                        hull_pen_percent /= 2;
                                    }
                                    break;
                                }
                                if rng.gen_range(0..100) < hull_pen_percent {
                                    self.components.remove_entity(entity_in_cell);
                                    self.spatial_table.remove(entity_in_cell);
                                }
                            }
                        }
                        self.projectile_stop(projectile_entity, external_events, rng);
                        return;
                    }
                }
                let _ignore_err = self
                    .spatial_table
                    .update_coord(projectile_entity, next_coord);
            } else {
                self.projectile_stop(projectile_entity, external_events, rng);
                return;
            }
        } else {
            self.components.remove_entity(projectile_entity);
            self.realtime_components.remove_entity(projectile_entity);
            self.spatial_table.remove(projectile_entity);
        }
    }

    fn character_push_in_direction(&mut self, entity: Entity, direction: Direction) {
        if let Some(current_coord) = self.spatial_table.coord_of(entity) {
            let target_coord = current_coord + direction.coord();
            if self.is_solid_feature_at_coord(target_coord) {
                return;
            }
            let _ignore_err = self.spatial_table.update_coord(entity, target_coord);
        }
    }

    fn character_die<R: Rng>(&mut self, character: Entity, rng: &mut R) {
        self.components.to_remove.insert(character, ());
    }

    pub fn damage_character<R: Rng>(
        &mut self,
        character: Entity,
        hit_points_to_lose: u32,
        rng: &mut R,
    ) {
        let hit_points = self
            .components
            .hit_points
            .get_mut(character)
            .expect("character lacks hit_points");
        if hit_points_to_lose >= hit_points.current {
            hit_points.current = 0;
            self.character_die(character, rng);
        } else {
            hit_points.current -= hit_points_to_lose;
        }
    }

    fn apply_projectile_damage<R: Rng>(
        &mut self,
        projectile_entity: Entity,
        mut projectile_damage: ProjectileDamage,
        projectile_movement_direction: Direction,
        entity_to_damage: Entity,
        rng: &mut R,
    ) {
        if let Some(armour) = self.components.armour.get(entity_to_damage).cloned() {
            if let Some(remaining_pen) = projectile_damage.pen.checked_sub(armour.value) {
                self.damage_character(entity_to_damage, projectile_damage.hit_points, rng);
                if projectile_damage.push_back {
                    self.character_push_in_direction(
                        entity_to_damage,
                        projectile_movement_direction,
                    );
                }
                if remaining_pen > 0 {
                    projectile_damage.pen = remaining_pen;
                    self.components
                        .projectile_damage
                        .insert(projectile_entity, projectile_damage);
                } else {
                    self.components.remove_entity(projectile_entity);
                }
            } else {
                self.components.remove_entity(projectile_entity);
            }
        }
    }

    pub fn apply_upgrade(
        &mut self,
        entity: Entity,
        upgrade: player::Upgrade,
    ) -> Result<(), ActionError> {
        let player = self.components.player.get_mut(entity).unwrap();
        if player.credit < upgrade.level.cost() {
            return Err(ActionError::CannotAffordUpgrade);
        }
        player.credit -= upgrade.level.cost();
        {
            let player_level = match upgrade.typ {
                player::UpgradeType::Toughness => &mut player.upgrade_table.toughness,
                player::UpgradeType::Accuracy => &mut player.upgrade_table.accuracy,
                player::UpgradeType::Endurance => &mut player.upgrade_table.endurance,
            };
            *player_level = Some(upgrade.level);
        }
        use player::{Upgrade, UpgradeLevel::*, UpgradeType::*};
        match upgrade {
            Upgrade {
                typ: Toughness,
                level: Level1,
            } => {
                player.ranged_weapons.push(None);
            }
            Upgrade {
                typ: Toughness,
                level: Level2,
            } => {
                let hit_points = self.components.hit_points.get_mut(entity).unwrap();
                hit_points.max *= 2;
                hit_points.current *= 2;
            }
            Upgrade {
                typ: Accuracy,
                level: Level1,
            } => {
                player.traits.reduce_hull_pen = true;
            }
            Upgrade {
                typ: Accuracy,
                level: Level2,
            } => {
                player.traits.double_damage = true;
            }
            Upgrade {
                typ: Endurance,
                level: Level1,
            } => {
                player.traits.half_vacuum_pull = true;
            }
            Upgrade {
                typ: Endurance,
                level: Level2,
            } => {
                let oxygen = self.components.oxygen.get_mut(entity).unwrap();
                oxygen.max *= 2;
                oxygen.current *= 2;
            }
        }
        Ok(())
    }

    pub fn equip_melee_weapon_from_ground(&mut self, entity: Entity) {
        if let Some(coord) = self.spatial_table.coord_of(entity) {
            if let Some((item_entity, weapon)) =
                self.spatial_table.layers_at(coord).and_then(|layers| {
                    layers.item.and_then(|item_entity| {
                        self.components
                            .weapon
                            .get(item_entity)
                            .map(|weapon| (item_entity, weapon.clone()))
                    })
                })
            {
                if weapon.is_melee() {
                    if let Some(player) = self.components.player.get_mut(entity) {
                        player.melee_weapon = weapon;
                        self.components.to_remove.insert(item_entity, ());
                    }
                }
            }
        }
    }

    pub fn equip_ranged_weapon_from_ground(
        &mut self,
        entity: Entity,
        slot: player::RangedWeaponSlot,
    ) {
        if let Some(coord) = self.spatial_table.coord_of(entity) {
            if let Some((item_entity, weapon)) =
                self.spatial_table.layers_at(coord).and_then(|layers| {
                    layers.item.and_then(|item_entity| {
                        self.components
                            .weapon
                            .get(item_entity)
                            .map(|weapon| (item_entity, weapon.clone()))
                    })
                })
            {
                if weapon.is_ranged() {
                    if let Some(player) = self.components.player.get_mut(entity) {
                        player.ranged_weapons[slot.index()] = Some(weapon);
                        self.components.to_remove.insert(item_entity, ());
                    }
                }
            }
        }
    }
}
