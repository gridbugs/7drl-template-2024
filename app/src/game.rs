use crate::audio::{AppAudioPlayer, AppHandle, Audio, AudioTable};
use crate::controls::{AppInput, Controls};
use crate::frontend::Frontend;
use crate::render::{GameToRender, GameView, Mode};
use chargrid::event_routine::common_event::*;
use chargrid::event_routine::*;
use chargrid::input::*;
use chargrid::render::{Rgb24, Style};
use chargrid::text::*;
use direction::{CardinalDirection, Direction};
use general_audio_static::{AudioHandle, AudioPlayer};
use general_storage_static::{format, StaticStorage};
use orbital_decay_game::{
    player, player::RangedWeaponSlot, ActionError, CharacterInfo, ExternalEvent, Game,
    GameControlFlow, Music,
};
pub use orbital_decay_game::{Config as GameConfig, Input as GameInput, Omniscient};
use rand::{Rng, SeedableRng};
use rand_isaac::Isaac64Rng;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const CONFIG_KEY: &str = "config.json";

const GAME_MUSIC_VOLUME: f32 = 0.05;
const MENU_MUSIC_VOLUME: f32 = 0.02;

const STORAGE_FORMAT: format::Bincode = format::Bincode;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Config {
    pub music: bool,
    pub sfx: bool,
    pub fullscreen: bool,
    pub first_run: bool,
    pub won: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            music: true,
            sfx: true,
            fullscreen: false,
            first_run: true,
            won: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct ScreenShake {
    remaining_frames: u8,
    direction: Direction,
}

impl ScreenShake {
    fn _coord(&self) -> Coord {
        self.direction.coord()
    }
    fn next(self) -> Option<Self> {
        self.remaining_frames
            .checked_sub(1)
            .map(|remaining_frames| Self {
                remaining_frames,
                direction: self.direction,
            })
    }
}

struct EffectContext<'a> {
    rng: &'a mut Isaac64Rng,
    screen_shake: &'a mut Option<ScreenShake>,
    current_music: &'a mut Option<Music>,
    current_music_handle: &'a mut Option<AppHandle>,
    audio_player: &'a AppAudioPlayer,
    audio_table: &'a AudioTable,
    player_coord: GameCoord,
    config: &'a Config,
}

impl<'a> EffectContext<'a> {
    fn next_frame(&mut self) {
        *self.screen_shake = self
            .screen_shake
            .and_then(|screen_shake| screen_shake.next());
    }
    fn play_audio(&self, audio: Audio, volume: f32) {
        log::info!("Playing audio {:?} at volume {:?}", audio, volume);
        let sound = self.audio_table.get(audio);
        let handle = self.audio_player.play(&sound);
        handle.set_volume(volume);
        handle.background();
    }
    fn handle_event(&mut self, event: ExternalEvent) {
        match event {
            ExternalEvent::Explosion(coord) => {
                let direction: Direction = self.rng.gen();
                *self.screen_shake = Some(ScreenShake {
                    remaining_frames: 2,
                    direction,
                });
                if self.config.sfx {
                    const BASE_VOLUME: f32 = 50.;
                    let distance_squared = (self.player_coord.0 - coord).magnitude2();
                    let volume = (BASE_VOLUME / (distance_squared as f32).max(1.)).min(1.);
                    self.play_audio(Audio::Explosion, volume);
                }
            }
            ExternalEvent::LoopMusic(music) => {
                *self.current_music = Some(music);
                let handle = loop_music(self.audio_player, self.audio_table, self.config, music);
                *self.current_music_handle = Some(handle);
            }
            ExternalEvent::SoundEffect(sound_effect) => {
                self.play_audio(Audio::SoundEffect(sound_effect), 30.);
            }
        }
    }
}

fn loop_music(
    audio_player: &AppAudioPlayer,
    audio_table: &AudioTable,
    config: &Config,
    music: Music,
) -> AppHandle {
    let audio = match music {
        Music::Gameplay0 => Audio::Gameplay0,
        Music::Gameplay1 => Audio::Gameplay1,
        Music::Gameplay2 => Audio::Gameplay2,
        Music::Boss => Audio::Boss,
    };
    let volume = GAME_MUSIC_VOLUME;
    log::info!("Looping audio {:?} at volume {:?}", audio, volume);
    let sound = audio_table.get(audio);
    let handle = audio_player.play_loop(&sound);
    handle.set_volume(volume);
    if !config.music {
        handle.pause();
    }
    handle
}

pub enum InjectedInput {
    Fire(Fire),
    Upgrade(player::Upgrade),
    GetMeleeWeapon,
    GetRangedWeapon(RangedWeaponSlot),
}

#[derive(Clone, Copy)]
pub struct ScreenCoord(pub Coord);

#[derive(Clone, Copy)]
struct GameCoord(Coord);

#[derive(Clone, Copy)]
struct PlayerCoord(Coord);

impl GameCoord {
    fn of_player(player_info: &CharacterInfo) -> Self {
        Self(player_info.coord)
    }
}

#[derive(Serialize, Deserialize)]
pub struct GameInstance {
    rng: Isaac64Rng,
    game: Game,
    screen_shake: Option<ScreenShake>,
    current_music: Option<Music>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum GameStatus {
    Playing,
    Dead,
    Adrift,
}

#[derive(Clone, Copy, Debug)]
pub enum RngSeed {
    Random,
    U64(u64),
}

impl GameInstance {
    fn new(game_config: &GameConfig, mut rng: Isaac64Rng) -> Self {
        Self {
            game: Game::new(game_config, &mut rng),
            rng,
            screen_shake: None,
            current_music: None,
        }
    }
    pub fn game(&self) -> &Game {
        &self.game
    }
}

pub struct GameData {
    instance: Option<GameInstance>,
    controls: Controls,
    rng_seed_source: RngSeedSource,
    last_aim_with_mouse: bool,
    storage_wrapper: StorageWrapper,
    audio_player: AppAudioPlayer,
    audio_table: AudioTable,
    game_config: GameConfig,
    frontend: Frontend,
    music_handle: Option<AppHandle>,
    config: Config,
}

struct StorageWrapper {
    storage: StaticStorage,
    save_key: String,
}

impl StorageWrapper {
    pub fn save_instance(&mut self, instance: &GameInstance) {
        self.storage
            .store(&self.save_key, instance, STORAGE_FORMAT)
            .expect("failed to save instance");
    }
    pub fn clear_instance(&mut self) {
        let _ = self.storage.remove(&self.save_key);
    }
}

struct RngSeedSource {
    rng: Isaac64Rng,
    next: u64,
}

impl RngSeedSource {
    fn new(rng_seed: RngSeed) -> Self {
        let mut rng = Isaac64Rng::from_entropy();
        let next = match rng_seed {
            RngSeed::Random => rng.gen(),
            RngSeed::U64(seed) => seed,
        };
        Self { rng, next }
    }
    fn next_seed(&mut self) -> u64 {
        let seed = self.next;
        self.next = self.rng.gen();
        seed
    }
}

impl GameData {
    pub fn new(
        game_config: GameConfig,
        controls: Controls,
        storage: StaticStorage,
        save_key: String,
        audio_player: AppAudioPlayer,
        rng_seed: RngSeed,
        frontend: Frontend,
    ) -> Self {
        let config = storage.load(CONFIG_KEY, format::Json).unwrap_or_default();
        let mut instance: Option<GameInstance> = match storage.load(&save_key, STORAGE_FORMAT) {
            Ok(instance) => Some(instance),
            Err(e) => {
                log::info!("no instance found: {:?}", e);
                None
            }
        };
        if let Some(instance) = instance.as_mut() {
            instance.game.update_visibility(&game_config);
        }
        let rng_seed_source = RngSeedSource::new(rng_seed);
        let storage_wrapper = StorageWrapper { storage, save_key };
        let audio_table = AudioTable::new(&audio_player);
        let music_handle = if let Some(instance) = instance.as_ref() {
            if let Some(music) = instance.current_music {
                let handle = loop_music(&audio_player, &audio_table, &config, music);
                Some(handle)
            } else {
                None
            }
        } else {
            None
        };
        Self {
            instance,
            controls,
            rng_seed_source,
            last_aim_with_mouse: false,
            storage_wrapper,
            audio_table,
            audio_player,
            game_config,
            frontend,
            music_handle,
            config,
        }
    }
    pub fn is_music_playing(&self) -> bool {
        self.music_handle.is_some()
    }
    pub fn loop_music(&mut self, audio: Audio, volume: f32) {
        log::info!("Looping audio {:?} at volume {:?}", audio, volume);
        let sound = self.audio_table.get(audio);
        let handle = self.audio_player.play_loop(&sound);
        handle.set_volume(volume);
        if !self.config.music {
            handle.pause();
        }
        self.music_handle = Some(handle);
    }
    pub fn config(&self) -> Config {
        self.config
    }
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
        if let Some(music_handle) = self.music_handle.as_ref() {
            if config.music {
                music_handle.play();
            } else {
                music_handle.pause();
            }
        }
        let _ = self
            .storage_wrapper
            .storage
            .store(CONFIG_KEY, &config, format::Json);
    }
    pub fn pre_game_loop(&mut self) {
        if let Some(music_handle) = self.music_handle.as_ref() {
            music_handle.set_volume(GAME_MUSIC_VOLUME);
            if self.config.music {
                music_handle.play();
            }
        }
    }
    pub fn post_game_loop(&mut self) {
        if self.instance.is_some() {
            if let Some(music_handle) = self.music_handle.as_ref() {
                music_handle.set_volume(MENU_MUSIC_VOLUME);
            }
        }
    }
    pub fn has_instance(&self) -> bool {
        self.instance.is_some()
    }
    pub fn instantiate(&mut self) {
        let seed = self.rng_seed_source.next_seed();
        self.frontend.log_rng_seed(seed);
        let rng = Isaac64Rng::seed_from_u64(seed);
        self.instance = Some(GameInstance::new(&self.game_config, rng));
    }
    pub fn save_instance(&mut self) {
        log::info!("saving game...");
        if let Some(instance) = self.instance.as_ref() {
            self.storage_wrapper.save_instance(instance);
        } else {
            self.storage_wrapper.clear_instance();
        }
    }
    pub fn clear_instance(&mut self) {
        self.instance = None;
        self.storage_wrapper.clear_instance();
        self.music_handle = None;
    }
    pub fn instance(&self) -> Option<&GameInstance> {
        self.instance.as_ref()
    }
    pub fn initial_aim_coord(
        &self,
        screen_coord_of_mouse: ScreenCoord,
    ) -> Result<ScreenCoord, NoGameInstance> {
        if let Some(instance) = self.instance.as_ref() {
            if self.last_aim_with_mouse {
                Ok(screen_coord_of_mouse)
            } else {
                let player_coord = GameCoord::of_player(instance.game.player_info());
                Ok(ScreenCoord(player_coord.0 * 3))
            }
        } else {
            Err(NoGameInstance)
        }
    }
}

pub struct NoGameInstance;

pub struct ExamineEventRoutine {
    screen_coord: Coord,
}

impl ExamineEventRoutine {
    pub fn new(screen_coord: Coord) -> Self {
        Self { screen_coord }
    }
}

impl EventRoutine for ExamineEventRoutine {
    type Return = ();
    type Data = GameData;
    type View = GameView;
    type Event = CommonEvent;

    fn handle<EP>(
        self,
        data: &mut Self::Data,
        view: &Self::View,
        event_or_peek: EP,
    ) -> Handled<Self::Return, Self>
    where
        EP: EventOrPeek<Event = Self::Event>,
    {
        enum Examine {
            Frame(Duration),
            Ignore,
            Cancel,
            Mouse { coord: Coord, press: bool },
            KeyboardDirection(CardinalDirection),
        }
        let last_aim_with_mouse = &mut data.last_aim_with_mouse;
        let controls = &data.controls;
        let audio_player = &data.audio_player;
        let audio_table = &data.audio_table;
        let game_config = &data.game_config;
        let current_music_handle = &mut data.music_handle;
        let config = &data.config;
        if let Some(instance) = data.instance.as_mut() {
            event_or_peek_with_handled(event_or_peek, self, |mut s, event| {
                let examine = match event {
                    CommonEvent::Input(input) => match input {
                        Input::Gamepad(_) => Examine::Ignore,
                        Input::Keyboard(keyboard_input) => {
                            if let Some(app_input) = controls.get(keyboard_input) {
                                match app_input {
                                    AppInput::Move(direction) => {
                                        Examine::KeyboardDirection(direction)
                                    }
                                    AppInput::Examine => Examine::Cancel,
                                    AppInput::Wait | AppInput::Aim(_) | AppInput::Get => {
                                        Examine::Ignore
                                    }
                                }
                            } else {
                                match keyboard_input {
                                    keys::ESCAPE => Examine::Cancel,
                                    _ => Examine::Ignore,
                                }
                            }
                        }
                        Input::Mouse(mouse_input) => match mouse_input {
                            MouseInput::MouseMove { coord, .. } => Examine::Mouse {
                                coord,
                                press: false,
                            },
                            MouseInput::MousePress {
                                coord,
                                button: MouseButton::Left,
                            } => Examine::Mouse { coord, press: true },
                            MouseInput::MousePress {
                                button: MouseButton::Right,
                                ..
                            } => Examine::Cancel,
                            _ => Examine::Ignore,
                        },
                    },
                    CommonEvent::Frame(since_last) => Examine::Frame(since_last),
                };
                match examine {
                    Examine::KeyboardDirection(direction) => {
                        *last_aim_with_mouse = false;
                        s.screen_coord += direction.coord() * 3;
                        Handled::Continue(s)
                    }
                    Examine::Mouse { coord, press } => {
                        s.screen_coord = view.absolute_coord_to_game_relative_screen_coord(coord);
                        *last_aim_with_mouse = true;
                        if press {
                            Handled::Return(())
                        } else {
                            Handled::Continue(s)
                        }
                    }
                    Examine::Cancel => Handled::Return(()),
                    Examine::Ignore => Handled::Continue(s),
                    Examine::Frame(since_last) => {
                        let game_control_flow = instance.game.handle_tick(since_last, game_config);
                        assert!(game_control_flow.is_none(), "meaningful event while aiming");
                        let mut event_context = EffectContext {
                            rng: &mut instance.rng,
                            screen_shake: &mut instance.screen_shake,
                            current_music: &mut instance.current_music,
                            current_music_handle,
                            audio_player,
                            audio_table,
                            player_coord: GameCoord::of_player(instance.game.player_info()),
                            config,
                        };
                        event_context.next_frame();
                        for event in instance.game.events() {
                            event_context.handle_event(event);
                        }
                        Handled::Continue(s)
                    }
                }
            })
        } else {
            Handled::Return(())
        }
    }

    fn view<F, C>(
        &self,
        data: &Self::Data,
        view: &mut Self::View,
        context: ViewContext<C>,
        frame: &mut F,
    ) where
        F: Frame,
        C: ColModify,
    {
        if let Some(instance) = data.instance.as_ref() {
            view.view(
                GameToRender {
                    game: &instance.game,
                    status: GameStatus::Playing,
                    mouse_coord: Some(self.screen_coord),
                    mode: Mode::Examine {
                        target: self.screen_coord,
                    },
                    action_error: None,
                },
                context,
                frame,
            );
        }
    }
}

#[derive(Clone, Copy)]
pub struct Fire {
    direction: CardinalDirection,
    slot: RangedWeaponSlot,
}

pub struct AimEventRoutine {
    screen_coord: Option<ScreenCoord>,
    duration: Duration,
    slot: RangedWeaponSlot,
}

impl AimEventRoutine {
    pub fn new(screen_coord: ScreenCoord, slot: RangedWeaponSlot) -> Self {
        Self {
            screen_coord: None,
            duration: Duration::from_millis(0),
            slot,
        }
    }
}

impl EventRoutine for AimEventRoutine {
    type Return = Option<Fire>;
    type Data = GameData;
    type View = GameView;
    type Event = CommonEvent;

    fn handle<EP>(
        self,
        data: &mut Self::Data,
        view: &Self::View,
        event_or_peek: EP,
    ) -> Handled<Self::Return, Self>
    where
        EP: EventOrPeek<Event = Self::Event>,
    {
        enum Aim {
            KeyboardDirection(CardinalDirection),
            KeyboardFinalise(CardinalDirection),
            Cancel,
            Ignore,
            Frame(Duration),
        }
        let last_aim_with_mouse = &mut data.last_aim_with_mouse;
        let controls = &data.controls;
        let audio_player = &data.audio_player;
        let audio_table = &data.audio_table;
        let game_config = &data.game_config;
        let current_music_handle = &mut data.music_handle;
        let config = &data.config;
        let slot = self.slot;
        if let Some(instance) = data.instance.as_mut() {
            event_or_peek_with_handled(event_or_peek, self, |mut s, event| {
                let aim = match event {
                    CommonEvent::Input(input) => match input {
                        Input::Gamepad(_) => Aim::Ignore,
                        Input::Keyboard(keyboard_input) => {
                            if let Some(app_input) = controls.get(keyboard_input) {
                                match app_input {
                                    AppInput::Move(direction) => Aim::KeyboardFinalise(direction),
                                    AppInput::Wait | AppInput::Examine | AppInput::Get => {
                                        Aim::Ignore
                                    }
                                    AppInput::Aim(slot) => {
                                        s.slot = slot;
                                        Aim::Ignore
                                    }
                                }
                            } else {
                                match keyboard_input {
                                    keys::ESCAPE => Aim::Cancel,
                                    _ => Aim::Ignore,
                                }
                            }
                        }
                        Input::Mouse(MouseInput::MouseMove { coord, .. }) => {
                            s.screen_coord = Some(ScreenCoord(coord));
                            Aim::Ignore
                        }
                        Input::Mouse(_) => Aim::Ignore,
                    },
                    CommonEvent::Frame(since_last) => Aim::Frame(since_last),
                };
                match aim {
                    Aim::KeyboardFinalise(direction) => {
                        *last_aim_with_mouse = false;
                        Handled::Return(Some(Fire { direction, slot }))
                    }
                    Aim::KeyboardDirection(direction) => {
                        *last_aim_with_mouse = false;
                        Handled::Continue(s)
                    }
                    Aim::Cancel => Handled::Return(None),
                    Aim::Ignore => Handled::Continue(s),
                    Aim::Frame(since_last) => {
                        let game_control_flow = instance.game.handle_tick(since_last, game_config);
                        assert!(game_control_flow.is_none(), "meaningful event while aiming");
                        let mut event_context = EffectContext {
                            rng: &mut instance.rng,
                            screen_shake: &mut instance.screen_shake,
                            current_music: &mut instance.current_music,
                            current_music_handle,
                            audio_player,
                            audio_table,
                            player_coord: GameCoord::of_player(instance.game.player_info()),
                            config,
                        };
                        event_context.next_frame();
                        for event in instance.game.events() {
                            event_context.handle_event(event);
                        }
                        s.duration += since_last;
                        Handled::Continue(s)
                    }
                }
            })
        } else {
            Handled::Return(None)
        }
    }

    fn view<F, C>(
        &self,
        data: &Self::Data,
        view: &mut Self::View,
        context: ViewContext<C>,
        frame: &mut F,
    ) where
        F: Frame,
        C: ColModify,
    {
        if let Some(instance) = data.instance.as_ref() {
            view.view(
                GameToRender {
                    game: &instance.game,
                    status: GameStatus::Playing,
                    mouse_coord: self.screen_coord.map(|s| s.0),
                    mode: Mode::Aim { slot: self.slot },
                    action_error: None,
                },
                context,
                frame,
            );
        }
    }
}

pub struct ChooseWeaponSlotEventRoutine;
impl EventRoutine for ChooseWeaponSlotEventRoutine {
    type Return = Option<RangedWeaponSlot>;
    type Data = GameData;
    type View = GameView;
    type Event = CommonEvent;

    fn handle<EP>(
        self,
        data: &mut Self::Data,
        view: &Self::View,
        event_or_peek: EP,
    ) -> Handled<Self::Return, Self>
    where
        EP: EventOrPeek<Event = Self::Event>,
    {
        let controls = &data.controls;
        event_or_peek_with_handled(event_or_peek, self, |mut s, event| match event {
            CommonEvent::Input(input) => match input {
                Input::Keyboard(keyboard_input) => {
                    if let Some(app_input) = controls.get(keyboard_input) {
                        match app_input {
                            AppInput::Aim(slot) => {
                                if let RangedWeaponSlot::Slot3 = slot {
                                    if !data
                                        .instance
                                        .as_ref()
                                        .unwrap()
                                        .game
                                        .player_has_third_weapon_slot()
                                    {
                                        return Handled::Return(None);
                                    }
                                }
                                Handled::Return(Some(slot))
                            }
                            _ => Handled::Continue(s),
                        }
                    } else {
                        match keyboard_input {
                            keys::ESCAPE => Handled::Return(None),
                            _ => Handled::Continue(s),
                        }
                    }
                }
                _ => Handled::Continue(s),
            },
            _ => Handled::Continue(s),
        })
    }

    fn view<F, C>(
        &self,
        data: &Self::Data,
        view: &mut Self::View,
        context: ViewContext<C>,
        frame: &mut F,
    ) where
        F: Frame,
        C: ColModify,
    {
        if let Some(instance) = data.instance.as_ref() {
            view.view(
                GameToRender {
                    game: &instance.game,
                    status: GameStatus::Playing,
                    mouse_coord: None,
                    mode: Mode::Normal,
                    action_error: None,
                },
                context,
                frame,
            );
            let num_weapon_slots = if instance.game.player_has_third_weapon_slot() {
                3
            } else {
                2
            };
            let text = format!(
                "Choose a weapon slot: (press 1-{} or escape to cancel)",
                num_weapon_slots
            );
            StringViewSingleLine::new(
                Style::new()
                    .with_foreground(Rgb24::new(255, 0, 0))
                    .with_bold(true),
            )
            .view(
                text.as_str(),
                context.add_offset(Coord { x: 0, y: 1 }),
                frame,
            );
        }
    }
}

pub struct GameEventRoutine {
    injected_inputs: Vec<InjectedInput>,
    mouse_coord: Option<Coord>,
    action_error: Option<ActionError>,
}

impl GameEventRoutine {
    pub fn new() -> Self {
        Self::new_injecting_inputs(Vec::new())
    }
    pub fn new_injecting_inputs(injected_inputs: Vec<InjectedInput>) -> Self {
        Self {
            injected_inputs,
            mouse_coord: None,
            action_error: None,
        }
    }
}

pub enum GameReturn {
    Pause,
    Aim(RangedWeaponSlot),
    GameOver,
    Win,
    Examine,
    Upgrade,
    EquipRanged,
    ConfirmReplaceMelee,
}

impl EventRoutine for GameEventRoutine {
    type Return = GameReturn;
    type Data = GameData;
    type View = GameView;
    type Event = CommonEvent;

    fn handle<EP>(
        mut self,
        data: &mut Self::Data,
        _view: &Self::View,
        event_or_peek: EP,
    ) -> Handled<Self::Return, Self>
    where
        EP: EventOrPeek<Event = Self::Event>,
    {
        let storage_wrapper = &mut data.storage_wrapper;
        let audio_player = &data.audio_player;
        let audio_table = &data.audio_table;
        let game_config = &data.game_config;
        let current_music_handle = &mut data.music_handle;
        let config = &data.config;
        if let Some(instance) = data.instance.as_mut() {
            let player_coord = GameCoord::of_player(instance.game.player_info());
            for injected_input in self.injected_inputs.drain(..) {
                match injected_input {
                    InjectedInput::Fire(Fire { direction, slot }) => {
                        let _ = instance
                            .game
                            .handle_input(GameInput::Fire { direction, slot }, game_config);
                    }
                    InjectedInput::Upgrade(upgrade) => {
                        let _ = instance
                            .game
                            .handle_input(GameInput::Upgrade(upgrade), game_config);
                    }
                    InjectedInput::GetMeleeWeapon => {
                        let _ = instance
                            .game
                            .handle_input(GameInput::EquipMeleeWeapon, game_config);
                    }
                    InjectedInput::GetRangedWeapon(slot) => {
                        let _ = instance
                            .game
                            .handle_input(GameInput::EquipRangedWeapon(slot), game_config);
                    }
                }
            }
            let controls = &data.controls;
            event_or_peek_with_handled(event_or_peek, self, |mut s, event| match event {
                CommonEvent::Input(input) => {
                    match input {
                        Input::Gamepad(gamepad_input) => match gamepad_input.button {
                            GamepadButton::Start => return Handled::Return(GameReturn::Pause),
                            other => {
                                if !instance.game.is_gameplay_blocked() {
                                    if let Some(app_input) = controls.get_gamepad(other) {
                                        let game_control_flow = match app_input {
                                            AppInput::Move(direction) => {
                                                instance.game.handle_input(
                                                    GameInput::Walk(direction),
                                                    game_config,
                                                )
                                            }
                                            _ => Ok(None),
                                        };
                                        match game_control_flow {
                                            Err(error) => s.action_error = Some(error),
                                            Ok(None) => s.action_error = None,
                                            Ok(Some(game_control_flow)) => {
                                                match game_control_flow {
                                                    GameControlFlow::Win => {
                                                        return Handled::Return(GameReturn::Win)
                                                    }
                                                    GameControlFlow::GameOver => {
                                                        return Handled::Return(
                                                            GameReturn::GameOver,
                                                        )
                                                    }
                                                    GameControlFlow::LevelChange => {
                                                        return Handled::Continue(s);
                                                    }
                                                    GameControlFlow::Upgrade => {
                                                        return Handled::Return(GameReturn::Upgrade)
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        Input::Keyboard(keyboard_input) => {
                            if keyboard_input == keys::ESCAPE {
                                return Handled::Return(GameReturn::Pause);
                            }
                            if !instance.game.is_gameplay_blocked() {
                                if let Some(app_input) = controls.get(keyboard_input) {
                                    let game_control_flow = match app_input {
                                        AppInput::Move(direction) => instance
                                            .game
                                            .handle_input(GameInput::Walk(direction), game_config),
                                        AppInput::Wait => {
                                            instance.game.handle_input(GameInput::Wait, game_config)
                                        }
                                        AppInput::Examine => {
                                            return Handled::Return(GameReturn::Examine)
                                        }
                                        AppInput::Aim(slot) => {
                                            if instance.game.player_has_usable_weapon_in_slot(slot)
                                            {
                                                return Handled::Return(GameReturn::Aim(slot));
                                            }
                                            Ok(None)
                                        }
                                        AppInput::Get => {
                                            if let Some(weapon) =
                                                instance.game.weapon_under_player()
                                            {
                                                if weapon.is_ranged() {
                                                    return Handled::Return(
                                                        GameReturn::EquipRanged,
                                                    );
                                                }
                                                if weapon.is_melee() {
                                                    return Handled::Return(
                                                        GameReturn::ConfirmReplaceMelee,
                                                    );
                                                } else {
                                                    Ok(None)
                                                }
                                            } else {
                                                Ok(None)
                                            }
                                        }
                                    };
                                    match game_control_flow {
                                        Err(error) => s.action_error = Some(error),
                                        Ok(None) => s.action_error = None,
                                        Ok(Some(game_control_flow)) => match game_control_flow {
                                            GameControlFlow::Win => {
                                                return Handled::Return(GameReturn::Win)
                                            }
                                            GameControlFlow::GameOver => {
                                                return Handled::Return(GameReturn::GameOver)
                                            }
                                            GameControlFlow::LevelChange => {
                                                return Handled::Continue(s);
                                            }
                                            GameControlFlow::Upgrade => {
                                                return Handled::Return(GameReturn::Upgrade)
                                            }
                                        },
                                    }
                                }
                            }
                        }
                        Input::Mouse(mouse_input) => match mouse_input {
                            MouseInput::MouseMove { coord, .. } => {
                                s.mouse_coord = Some(coord);
                            }
                            _ => (),
                        },
                    }
                    Handled::Continue(s)
                }
                CommonEvent::Frame(period) => {
                    let maybe_control_flow = instance.game.handle_tick(period, game_config);
                    let mut event_context = EffectContext {
                        rng: &mut instance.rng,
                        screen_shake: &mut instance.screen_shake,
                        current_music: &mut instance.current_music,
                        current_music_handle,
                        audio_player,
                        audio_table,
                        player_coord,
                        config,
                    };
                    event_context.next_frame();
                    for event in instance.game.events() {
                        event_context.handle_event(event);
                    }
                    if let Some(game_control_flow) = maybe_control_flow {
                        match game_control_flow {
                            GameControlFlow::Win => return Handled::Return(GameReturn::Win),
                            GameControlFlow::GameOver => {
                                return Handled::Return(GameReturn::GameOver)
                            }
                            GameControlFlow::LevelChange => {
                                return Handled::Continue(s);
                            }
                            GameControlFlow::Upgrade => {
                                return Handled::Return(GameReturn::Upgrade)
                            }
                        }
                    }
                    Handled::Continue(s)
                }
            })
        } else {
            storage_wrapper.clear_instance();
            Handled::Continue(self)
        }
    }

    fn view<F, C>(
        &self,
        data: &Self::Data,
        view: &mut Self::View,
        context: ViewContext<C>,
        frame: &mut F,
    ) where
        F: Frame,
        C: ColModify,
    {
        if let Some(instance) = data.instance.as_ref() {
            view.view(
                GameToRender {
                    game: &instance.game,
                    status: GameStatus::Playing,
                    mouse_coord: self.mouse_coord,
                    mode: Mode::Normal,
                    action_error: self.action_error,
                },
                context,
                frame,
            );
        }
    }
}

pub struct GameOverEventRoutine {
    duration: Duration,
}

impl GameOverEventRoutine {
    pub fn new() -> Self {
        Self {
            duration: Duration::from_millis(0),
        }
    }
}

impl EventRoutine for GameOverEventRoutine {
    type Return = ();
    type Data = GameData;
    type View = GameView;
    type Event = CommonEvent;

    fn handle<EP>(
        self,
        data: &mut Self::Data,
        _view: &Self::View,
        event_or_peek: EP,
    ) -> Handled<Self::Return, Self>
    where
        EP: EventOrPeek<Event = Self::Event>,
    {
        let game_config = &data.game_config;
        let audio_player = &data.audio_player;
        let audio_table = &data.audio_table;
        let current_music_handle = &mut data.music_handle;
        let config = &data.config;
        if let Some(instance) = data.instance.as_mut() {
            event_or_peek_with_handled(event_or_peek, self, |mut s, event| match event {
                CommonEvent::Input(input) => match input {
                    Input::Keyboard(_) | Input::Gamepad(_) => Handled::Return(()),
                    Input::Mouse(_) => Handled::Continue(s),
                },
                CommonEvent::Frame(period) => {
                    s.duration += period;
                    const NPC_TURN_PERIOD: Duration = Duration::from_millis(100);
                    if s.duration > NPC_TURN_PERIOD {
                        s.duration -= NPC_TURN_PERIOD;
                        instance.game.handle_npc_turn();
                    }
                    let _ = instance.game.handle_tick(period, game_config);
                    let mut event_context = EffectContext {
                        rng: &mut instance.rng,
                        screen_shake: &mut instance.screen_shake,
                        current_music: &mut instance.current_music,
                        current_music_handle,
                        audio_player,
                        audio_table,
                        player_coord: GameCoord::of_player(instance.game.player_info()),
                        config,
                    };
                    event_context.next_frame();
                    for event in instance.game.events() {
                        event_context.handle_event(event);
                    }
                    Handled::Continue(s)
                }
            })
        } else {
            Handled::Return(())
        }
    }
    fn view<F, C>(
        &self,
        data: &Self::Data,
        view: &mut Self::View,
        context: ViewContext<C>,
        frame: &mut F,
    ) where
        F: Frame,
        C: ColModify,
    {
        if let Some(instance) = data.instance.as_ref() {
            let status = if instance.game.is_adrift() {
                GameStatus::Adrift
            } else {
                GameStatus::Dead
            };
            view.view(
                GameToRender {
                    game: &instance.game,
                    status,
                    mouse_coord: None,
                    mode: Mode::Normal,
                    action_error: None,
                },
                context,
                frame,
            );
        }
    }
}
