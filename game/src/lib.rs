pub use direction::CardinalDirection;
pub use grid_2d::{Coord, Grid, Size};
use rand::{seq::SliceRandom, Rng, SeedableRng};
use rand_isaac::Isaac64Rng;
use serde::{Deserialize, Serialize};
use shadowcast::Context as ShadowcastContext;
use std::time::Duration;

mod behaviour;
mod terrain;
mod visibility;
mod world;

use behaviour::{Agent, BehaviourContext};
use entity_table::ComponentTable;
pub use entity_table::Entity;
pub use terrain::FINAL_LEVEL;
use terrain::{SpaceStationSpec, Terrain};
pub use visibility::{CellVisibility, Omniscient, VisibilityGrid};
use world::{make_player, AnimationContext, World, ANIMATION_FRAME_DURATION};
pub use world::{
    player, ActionError, CharacterInfo, EntityData, HitPoints, Layer, NpcAction, PlayerDied, Tile,
    ToRenderEntity,
};

pub const MAP_SIZE: Size = Size::new_u16(20, 14);

pub struct Config {
    pub omniscient: Option<Omniscient>,
    pub demo: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum Music {
    Gameplay0,
    Gameplay1,
    Gameplay2,
    Boss,
}

/// Events which the game can report back to the io layer so it can
/// respond with a sound/visual effect.
#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum ExternalEvent {
    Explosion(Coord),
    LoopMusic(Music),
}

pub enum GameControlFlow {
    GameOver,
    Win,
    LevelChange,
}

#[derive(Clone, Copy, Debug)]
pub enum Input {
    Walk(CardinalDirection),
    Wait,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum Turn {
    Player,
    Npc,
}

#[derive(Serialize, Deserialize)]
pub struct Game {
    world: World,
    visibility_grid: VisibilityGrid,
    player: Entity,
    last_player_info: CharacterInfo,
    rng: Isaac64Rng,
    animation_rng: Isaac64Rng,
    events: Vec<ExternalEvent>,
    shadowcast_context: ShadowcastContext<u8>,
    behaviour_context: BehaviourContext,
    animation_context: AnimationContext,
    agents: ComponentTable<Agent>,
    agents_to_remove: Vec<Entity>,
    since_last_frame: Duration,
    generate_frame_countdown: Option<Duration>,
    after_player_turn_countdown: Option<Duration>,
    before_npc_turn_cooldown: Option<Duration>,
    dead_player: Option<EntityData>,
    turn_during_animation: Option<Turn>,
    gameplay_music: Vec<Music>,
    star_rng_seed: u64,
}

impl Game {
    pub fn new<R: Rng>(config: &Config, base_rng: &mut R) -> Self {
        let mut rng = Isaac64Rng::seed_from_u64(base_rng.gen());
        let animation_rng = Isaac64Rng::seed_from_u64(base_rng.gen());
        let star_rng_seed = base_rng.gen();
        let debug = false;
        let Terrain {
            world,
            agents,
            player,
        } = if debug {
            terrain::from_str(include_str!("terrain.txt"), make_player(&mut rng), &mut rng)
        } else {
            terrain::space_station(
                0,
                make_player(&mut rng),
                &SpaceStationSpec { demo: config.demo },
                &mut rng,
            )
        };
        let last_player_info = world
            .character_info(player)
            .expect("couldn't get info for player");
        let mut gameplay_music = vec![Music::Gameplay0, Music::Gameplay1, Music::Gameplay2];
        gameplay_music.shuffle(&mut rng);
        let events = vec![ExternalEvent::LoopMusic(gameplay_music[0])];
        let mut game = Self {
            visibility_grid: VisibilityGrid::new(world.size()),
            player,
            last_player_info,
            rng,
            animation_rng,
            events,
            shadowcast_context: ShadowcastContext::default(),
            behaviour_context: BehaviourContext::new(world.size()),
            animation_context: AnimationContext::default(),
            agents,
            agents_to_remove: Vec::new(),
            world,
            since_last_frame: Duration::from_millis(0),
            generate_frame_countdown: None,
            after_player_turn_countdown: None,
            before_npc_turn_cooldown: None,
            dead_player: None,
            turn_during_animation: None,
            gameplay_music,
            star_rng_seed,
        };
        game.update_visibility(config);
        game.prime_npcs();
        game
    }
    pub fn star_rng_seed(&self) -> u64 {
        self.star_rng_seed
    }
    pub fn size(&self) -> Size {
        self.world.size()
    }
    fn cleanup(&mut self) {
        if let Some(PlayerDied(player_data)) = self.world.cleanup() {
            self.dead_player = Some(player_data);
        }
    }
    pub fn is_gameplay_blocked(&self) -> bool {
        self.world.is_gameplay_blocked()
    }
    pub fn update_visibility(&mut self, config: &Config) {
        if let Some(player_coord) = self.world.entity_coord(self.player) {
            self.visibility_grid.update(
                player_coord,
                &self.world,
                &mut self.shadowcast_context,
                config.omniscient,
            );
        }
    }
    fn update_behaviour(&mut self) {
        self.behaviour_context.update(self.player, &self.world);
    }

    #[must_use]
    pub fn handle_tick(
        &mut self,
        since_last_tick: Duration,
        config: &Config,
    ) -> Option<GameControlFlow> {
        if let Some(countdown) = self.generate_frame_countdown.as_mut() {
            if countdown.as_millis() == 0 {
                self.generate_level(config);
                self.generate_frame_countdown = None;
                return Some(GameControlFlow::LevelChange);
            } else {
                *countdown = if let Some(remaining) = countdown.checked_sub(since_last_tick) {
                    remaining
                } else {
                    Duration::from_millis(0)
                };
            }
            return None;
        }
        self.since_last_frame += since_last_tick;
        while let Some(remaining_since_last_frame) =
            self.since_last_frame.checked_sub(ANIMATION_FRAME_DURATION)
        {
            self.since_last_frame = remaining_since_last_frame;
            if let Some(game_control_flow) = self.handle_tick_inner(since_last_tick, config) {
                return Some(game_control_flow);
            }
        }
        None
    }
    fn handle_tick_inner(
        &mut self,
        since_last_tick: Duration,
        config: &Config,
    ) -> Option<GameControlFlow> {
        self.world.animation_tick(
            &mut self.animation_context,
            &mut self.events,
            &mut self.animation_rng,
        );
        if !self.is_gameplay_blocked() {
            if let Some(turn_during_animation) = self.turn_during_animation {
                if let Some(countdown) = self.after_player_turn_countdown.as_mut() {
                    if countdown.as_millis() == 0 {
                        self.after_player_turn_countdown = None;
                        self.after_turn();
                    } else {
                        *countdown = if let Some(remaining) = countdown.checked_sub(since_last_tick)
                        {
                            remaining
                        } else {
                            Duration::from_millis(0)
                        }
                    }
                    return None;
                }
                if let Some(countdown) = self.before_npc_turn_cooldown.as_mut() {
                    if countdown.as_millis() == 0 {
                        self.before_npc_turn_cooldown = None;
                    } else {
                        *countdown = if let Some(remaining) = countdown.checked_sub(since_last_tick)
                        {
                            remaining
                        } else {
                            Duration::from_millis(0)
                        }
                    }
                    return None;
                }
                if let Turn::Player = turn_during_animation {
                    self.npc_turn();
                }
                self.turn_during_animation = None;
            }
        }
        self.update_visibility(config);
        self.update_last_player_info();
        if self.is_game_over() {
            Some(GameControlFlow::GameOver)
        } else if self.is_game_won() {
            Some(GameControlFlow::Win)
        } else {
            None
        }
    }

    #[must_use]
    pub fn handle_input(
        &mut self,
        input: Input,
        config: &Config,
    ) -> Result<Option<GameControlFlow>, ActionError> {
        if self.generate_frame_countdown.is_some() {
            return Ok(None);
        }
        let mut change = false;
        if !self.is_gameplay_blocked() && self.turn_during_animation.is_none() {
            change = true;
            self.player_turn(input)?;
        }
        if change {
            self.update_last_player_info();
            self.update_visibility(config);
        }
        if self.is_game_over() {
            Ok(Some(GameControlFlow::GameOver))
        } else if self.is_game_won() {
            Ok(Some(GameControlFlow::Win))
        } else {
            Ok(None)
        }
    }
    pub fn handle_npc_turn(&mut self) {
        if !self.is_gameplay_blocked() {
            self.world.process_door_close_countdown();
            self.npc_turn();
        }
    }
    fn prime_npcs(&mut self) {
        self.update_behaviour();
    }

    fn player_turn(&mut self, input: Input) -> Result<(), ActionError> {
        let result = match input {
            Input::Walk(direction) => {
                self.world
                    .character_walk_in_direction(self.player, direction, &mut self.rng)
            }
            Input::Wait => {
                self.world.wait(self.player, &mut self.rng);
                Ok(())
            }
        };
        if result.is_ok() {
            if self.is_gameplay_blocked() {
                self.after_player_turn_countdown = Some(Duration::from_millis(0));
                self.before_npc_turn_cooldown = Some(Duration::from_millis(100));
            }
            self.turn_during_animation = Some(Turn::Player);
        }
        result
    }

    fn npc_turn(&mut self) {
        self.update_behaviour();
        for (entity, agent) in self.agents.iter_mut() {
            if !self.world.entity_exists(entity) {
                self.agents_to_remove.push(entity);
                continue;
            }
            let input = agent.act(
                entity,
                &self.world,
                self.player,
                &mut self.behaviour_context,
                &mut self.shadowcast_context,
                &mut self.rng,
            );
            match input {
                NpcAction::Walk(direction) => {
                    let _ =
                        self.world
                            .character_walk_in_direction(entity, direction, &mut self.rng);
                }
                NpcAction::Wait => (),
            }
        }
        self.update_last_player_info();
        for entity in self.agents_to_remove.drain(..) {
            self.agents.remove(entity);
        }
        self.after_turn();
    }
    fn generate_level(&mut self, config: &Config) {
        let player_data = self.world.clone_entity_data(self.player);
        let Terrain {
            world,
            agents,
            player,
        } = terrain::space_station(
            self.world.level + 1,
            player_data,
            &SpaceStationSpec { demo: config.demo },
            &mut self.rng,
        );
        self.visibility_grid = VisibilityGrid::new(world.size());
        self.world = world;
        self.agents = agents;
        self.player = player;
        self.update_last_player_info();
        self.update_visibility(config);
        self.prime_npcs();
        if self.world.level == terrain::FINAL_LEVEL {
            self.events.push(ExternalEvent::LoopMusic(Music::Boss));
        } else {
            self.events.push(ExternalEvent::LoopMusic(
                self.gameplay_music[self.world.level as usize % self.gameplay_music.len()],
            ));
        }
    }
    fn after_turn(&mut self) {
        self.cleanup();
        if let Some(player_coord) = self.world.entity_coord(self.player) {
            if let Some(_stairs_entity) = self.world.get_stairs_at_coord(player_coord) {
                self.generate_frame_countdown = Some(Duration::from_millis(200));
            }
        }
        for entity in self.world.components.npc.entities() {
            if !self.agents.contains(entity) {
                self.agents.insert(entity, Agent::new(self.world.size()));
            }
        }
        self.cleanup();
    }
    pub fn is_generating(&self) -> bool {
        if let Some(countdown) = self.generate_frame_countdown {
            countdown.as_millis() == 0
        } else {
            false
        }
    }
    pub fn events(&mut self) -> impl '_ + Iterator<Item = ExternalEvent> {
        self.events.drain(..)
    }
    pub fn player_info(&self) -> &CharacterInfo {
        &self.last_player_info
    }
    pub fn world_size(&self) -> Size {
        self.world.size()
    }
    pub fn to_render_entities<'a>(&'a self) -> impl 'a + Iterator<Item = ToRenderEntity> {
        self.world.to_render_entities()
    }
    pub fn visibility_grid(&self) -> &VisibilityGrid {
        &self.visibility_grid
    }
    pub fn contains_wall(&self, coord: Coord) -> bool {
        self.world.is_wall_at_coord(coord)
    }
    pub fn contains_floor(&self, coord: Coord) -> bool {
        self.world.is_floor_at_coord(coord)
    }
    fn update_last_player_info(&mut self) {
        if let Some(character_info) = self.world.character_info(self.player) {
            self.last_player_info = character_info;
        }
    }
    fn is_game_over(&self) -> bool {
        self.dead_player.is_some()
    }
    fn is_game_won(&self) -> bool {
        self.world.is_won()
    }
    pub fn player(&self) -> &player::Player {
        if let Some(player) = self.world.entity_player(self.player) {
            player
        } else {
            self.dead_player.as_ref().unwrap().player.as_ref().unwrap()
        }
    }
    pub fn player_coord(&self) -> Coord {
        self.last_player_info.coord
    }
    pub fn player_hit_points(&self) -> Coord {
        self.last_player_info.coord
    }
    pub fn current_level(&self) -> u32 {
        self.world.level
    }
}
