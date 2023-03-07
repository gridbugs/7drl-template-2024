use crate::{
    world::{
        data::{Boat, EntityData},
        spatial::{Layer, Location},
        World,
    },
    Entity,
};
use gridbugs::{
    coord_2d::{Coord, Size},
    entity_table::entity_data,
};
use procgen::{generate, Spec, WorldCell2};
use rand::Rng;
use vector::Radians;

pub struct Terrain {
    pub world: World,
    pub player_entity: Entity,
}

impl Terrain {
    fn text_island(offset: Coord, world: &mut World) {
        let txt = include_str!("island.txt");
        for (y, row) in txt.split('\n').enumerate() {
            for (x, ch) in row.chars().enumerate() {
                let coord = offset + Coord::new(x as i32, y as i32);
                match ch {
                    ' ' => (),
                    '%' => {
                        world.spawn_rock(coord);
                    }
                    '.' => {
                        world.spawn_floor(coord);
                    }
                    '#' => {
                        world.spawn_wall(coord);
                    }
                    '+' => {
                        world.spawn_door(coord);
                    }
                    _ => panic!("unexpected char: {}", ch),
                }
            }
        }
    }

    pub fn generate_debug<R: Rng>(player_data: EntityData, rng: &mut R) -> Self {
        let mut world = World::new(Size::new(1000, 500));
        let player_entity = world.insert_entity_data(
            Location {
                coord: Coord::new(50, 50),
                layer: Some(Layer::Character),
            },
            player_data,
        );
        let boat_data = entity_data! {
            boat: Boat::new(Radians(0f64)),
        };
        world.insert_entity_data(
            Location {
                coord: Coord::new(50, 50),
                layer: None,
            },
            boat_data,
        );
        Self::text_island(Coord::new(65, 40), &mut world);
        let water_visible_chance = 0.01f64;
        for coord in world.spatial_table.grid_size().coord_iter_row_major() {
            if world.spatial_table.layers_at_checked(coord).floor.is_none() {
                if rng.gen::<f64>() < water_visible_chance {
                    world.spawn_water1(coord);
                } else {
                    world.spawn_water2(coord);
                }
            }
        }
        Self {
            world,
            player_entity,
        }
    }

    pub fn generate<R: Rng>(player_data: EntityData, rng: &mut R) -> Self {
        let g = generate(
            &Spec {
                size: Size::new(150, 80),
            },
            rng,
        );
        let mut world = World::new(g.world2.grid.size());
        let player_entity = world.insert_entity_data(
            Location {
                coord: g.world2.spawn,
                layer: Some(Layer::Character),
            },
            player_data,
        );
        let boat_data = entity_data! {
            boat: Boat::new(Radians(0f64)),
        };
        world.insert_entity_data(
            Location {
                coord: g.world2.spawn,
                layer: None,
            },
            boat_data,
        );
        println!("spawn: {:?}", g.world2.spawn);
        println!("world size: {:?}", g.world2.grid.size());
        let water_visible_chance = 0.01f64;
        for (coord, &cell) in g.world2.grid.enumerate() {
            if *g.water_distance_map.distances.get_checked(coord) < 20 {
                match cell {
                    WorldCell2::Land => {
                        world.spawn_floor(coord);
                    }
                    WorldCell2::Water => {
                        if rng.gen::<f64>() < water_visible_chance {
                            world.spawn_water1(coord);
                        } else {
                            world.spawn_water2(coord);
                        }
                    }
                }
            }
        }
        Self {
            world,
            player_entity,
        }
    }
}
