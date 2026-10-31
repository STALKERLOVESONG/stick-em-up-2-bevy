//! Sound effects and music: MasterAudio as `sticks_sim::audio` models it, played through
//! Bevy's audio output.
//!
//! * Every MasterAudio call site of the original is wired: `ProjectileLauncher` firing sounds,
//!   `PlaySoundOnExplode`, `LifetimeSound` (with its fade-out when the projectile goes),
//!   `PlaySoundOnButtonPress` and the Level Select's "Paper Slide" (through
//!   [`crate::lobby::Lobby`]'s sound queue), and each scene's `EventSounds` music cue (the
//!   "Music" `PlaylistController` playing the named song, looped, cross-faded).
//! * A voice's volume each frame: its MasterAudio volume (`sticks_sim::audio::SoundBank`),
//!   any fade, the mixer gain of its bus's `AudioMixerGroup` (with every parent group), and for
//!   3D voices the logarithmic rolloff at the `AudioListener` (the level camera).
//!
//! * Vehicles' own looping `AudioSource`s (the helicopter engine, `heli_loop`): played while
//!   the vehicle's object is active (`playOnAwake` restarts it on reactivation), at the volume
//!   and pitch `HelicopterControl` sets, through its `OutputAudioMixerGroup`, with the source's
//!   rolloff.
//!
//! TODO(UNCERTAIN): no stereo panning of 3D voices; WAV clips need Bevy's `wav` feature;
//! playlist cross-fade is linear in volume. Level-end applause (`EventSounds` on the inactive
//! "Ender") waits for the level-end flow.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use bevy::time::Real;
use sticks_sim::audio::{Fade, SoundBank, distance_gain};
use sticks_sim::rng::GameRng;
use unity_compat::Vec3 as SimVec3;
use unity_data::audio::{ClipIndex, MasterAudioSpec, MusicCue, load_master_audio, scene_sounds};

use crate::camera::{LevelCamera, Rig};
use crate::config::GameConfig;
use crate::effects::{FadeOutSounds, PlaySound};
use crate::level::ActiveLevel;
use crate::state::AppState;

/// The MasterAudio set-up and its runtime state.
#[derive(Resource)]
pub struct MasterAudio {
    spec: Arc<MasterAudioSpec>,
    bank: SoundBank,
    rng: GameRng,
    clips: HashMap<PathBuf, Handle<AudioSource>>,
    /// Every clip by GUID (for plain `AudioSource`s outside MasterAudio).
    clip_index: ClipIndex,
    frame: i64,
}

/// A vehicle's looping `AudioSource`.
#[derive(Component)]
struct EngineVoice {
    vehicle: Entity,
}

/// One playing voice.
#[derive(Component)]
struct VoiceInstance {
    instance: u64,
    group: usize,
    voice: usize,
    /// The `AudioSource.volume` MasterAudio set (fades change it).
    volume: f32,
    fade: Fade,
    started: f32,
    pitch: f32,
    at: Option<SimVec3>,
    owner: Option<Entity>,
}

/// The "Music" playlist controller's current song.
#[derive(Component)]
struct MusicTrack {
    song: String,
    /// Song volume × playlist volume × master playlist volume × mixer gain.
    full: f32,
    /// Cross-fade: `level` goes towards `target` at `rate` per second.
    level: f32,
    target: f32,
    rate: f32,
}

/// The music cue of the Lobby scene (read once), and whether it was started since the Lobby
/// was entered.
#[derive(Resource, Default)]
struct LobbyMusic {
    cue: Option<Option<MusicCue>>,
    started: bool,
}

pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LobbyMusic>()
            .add_systems(Startup, load_audio)
            // The first `OnEnter(Lobby)` runs before `Startup` has loaded MasterAudio, so the
            // cue is started from `Update` once the set-up exists.
            .add_systems(OnEnter(AppState::Lobby), |mut music: ResMut<LobbyMusic>| music.started = false)
            .add_systems(Update, lobby_music.run_if(in_state(AppState::Lobby)))
            .add_systems(OnEnter(AppState::InGame), level_music)
            .add_systems(Update, (lobby_sounds.run_if(in_state(AppState::Lobby)), play_sounds, fade_out_owned, update_voices, update_music, vehicle_engines).chain());
    }
}

fn load_audio(mut commands: Commands, config: Res<GameConfig>) {
    match load_master_audio(&config.assets_dir()) {
        Ok(spec) => {
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(3, |d| d.subsec_nanos());
            let mut rng = GameRng::from_seed(seed);
            let bank = SoundBank::new(&spec, &mut rng);
            info!("MasterAudio: {} sound groups, {} playlists", spec.groups.len(), spec.playlists.len());
            let clip_index = ClipIndex::scan(&config.assets_dir());
            commands.insert_resource(MasterAudio { spec: Arc::new(spec), bank, rng, clips: HashMap::new(), clip_index, frame: 0 });
        }
        Err(e) => warn!("no MasterAudio set-up, the game is silent: {e}"),
    }
}

impl MasterAudio {
    fn clip(&mut self, path: &std::path::Path, assets: &AssetServer) -> Handle<AudioSource> {
        self.clips.entry(path.to_path_buf()).or_insert_with(|| assets.load(path.to_path_buf())).clone()
    }
}

/// The lobby scripts' queued sounds.
fn lobby_sounds(lobby: Option<ResMut<crate::lobby::Lobby>>, mut out: MessageWriter<PlaySound>) {
    let Some(mut lobby) = lobby else { return };
    for (group, at) in lobby.sounds.drain(..) {
        out.write(PlaySound { group, at: Some(at), owner: None });
    }
}

fn listener(cameras: &Query<&Rig, With<LevelCamera>>) -> Option<SimVec3> {
    cameras.iter().next().map(|rig| SimVec3::new(rig.position.x, rig.position.y, rig.position.z))
}

/// `MasterAudio.PlaySound3DAtTransform[AndForget]`.
fn play_sounds(
    mut commands: Commands,
    audio: Option<ResMut<MasterAudio>>,
    assets: Res<AssetServer>,
    real: Res<Time<Real>>,
    mut requests: MessageReader<PlaySound>,
    voices: Query<(Entity, &VoiceInstance)>,
) {
    let Some(mut audio) = audio else {
        requests.clear();
        return;
    };
    audio.frame += 1;
    let now = real.elapsed_secs();
    let audio = &mut *audio;
    for request in requests.read() {
        let frame = audio.frame;
        let spec = audio.spec.clone();
        let Some(play) = audio.bank.play(&spec, &request.group, now, frame, 1.0, &mut audio.rng) else { continue };
        if let Some(old) = play.replaces {
            for (entity, v) in &voices {
                if v.instance == old {
                    commands.entity(entity).try_despawn();
                }
            }
        }
        let group = &spec.groups[play.group];
        let voice = &group.voices[play.voice];
        let Some(path) = &voice.clip else { continue };
        let handle = audio.clip(path, &assets);
        let fade = match (voice.fades, voice.clip_length) {
            (Some((fade_in, fade_out)), Some(length)) => Fade::in_out(play.volume, fade_in, fade_out, length, play.pitch, voice.looping),
            _ => Fade::None,
        };
        let settings = if voice.looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN };
        commands.spawn((
            Name::new(format!("voice {}", voice.variation)),
            AudioPlayer::new(handle),
            // The real volume is set every frame once the sink exists.
            settings.with_volume(Volume::Linear(0.0)).with_speed(play.pitch),
            VoiceInstance { instance: play.instance, group: play.group, voice: play.voice, volume: play.volume, fade, started: now, pitch: play.pitch, at: request.at, owner: request.owner },
        ));
    }
}

/// `LifetimeSound.OnDisable` → `FadeOutNow()` (`useFades` voices only).
fn fade_out_owned(audio: Option<Res<MasterAudio>>, time: Res<Time>, real: Res<Time<Real>>, mut fades: MessageReader<FadeOutSounds>, mut voices: Query<&mut VoiceInstance>) {
    let Some(audio) = audio else {
        fades.clear();
        return;
    };
    let now = real.elapsed_secs();
    for f in fades.read() {
        for mut v in &mut voices {
            if v.owner != Some(f.owner) {
                continue;
            }
            let voice = &audio.spec.groups[v.group].voices[v.voice];
            let Some((_, fade_out)) = voice.fades else { continue };
            let clip_time = (now - v.started) * v.pitch;
            let current = v.volume;
            v.fade = Fade::out_early(fade_out, clip_time, voice.clip_length, voice.looping, current, time.delta_secs());
        }
    }
}

/// `SoundGroupVariationUpdater` fading, then the volume the output hears.
fn update_voices(
    audio: Option<Res<MasterAudio>>,
    real: Res<Time<Real>>,
    cameras: Query<&Rig, With<LevelCamera>>,
    mut voices: Query<(&mut VoiceInstance, Option<&mut AudioSink>)>,
    memory: Res<crate::lobby::LobbyMemory>,
) {
    let Some(audio) = audio else { return };
    // The settings page's sound volume (not in the original).
    let sound = memory.settings.sound_volume;
    let now = real.elapsed_secs();
    let ear = listener(&cameras);
    for (mut v, sink) in &mut voices {
        let group = &audio.spec.groups[v.group];
        let voice = &group.voices[v.voice];
        let clip_time = (now - v.started) * v.pitch;
        if let Some(length) = voice.clip_length {
            if let Some(volume) = v.fade.step(clip_time, length) {
                v.volume = volume;
            }
        }
        let Some(mut sink) = sink else { continue };
        let blend = audio.spec.spatial_blend(group, voice);
        let distance = match (v.at, ear) {
            (Some(at), Some(ear)) => distance_gain(at, ear, voice.min_distance, voice.max_distance, voice.rolloff, blend),
            _ => 1.0,
        };
        let gain = v.volume * audio.spec.mixer_gain(group) * distance * sound;
        sink.set_volume(Volume::Linear(gain.max(0.0)));
    }
}

fn start_music(commands: &mut Commands, audio: &mut MasterAudio, assets: &AssetServer, cue: &MusicCue, tracks: &mut Query<(Entity, &mut MusicTrack)>) {
    let Some(controller) = audio.spec.playlist_controller.clone().filter(|c| c.name == cue.playlist_controller) else { return };
    let Some(playlist) = audio.spec.playlists.iter().find(|p| p.name == controller.start_playlist).cloned() else { return };
    let Some(song) = playlist.songs.iter().find(|s| s.name == cue.song) else {
        warn!("no song {:?} in playlist {:?}", cue.song, playlist.name);
        return;
    };
    // Already playing it: nothing changes.
    if tracks.iter().any(|(_, t)| t.song == song.name && t.target > 0.0) {
        return;
    }
    let rate = 1.0 / playlist.crossfade_time.max(1e-3);
    let mut fading_in = false;
    for (_, mut t) in tracks.iter_mut() {
        t.target = 0.0;
        t.rate = rate;
        fading_in = true;
    }
    let Some(path) = &song.clip else { return };
    let handle = audio.clip(path, assets);
    let mixer = controller.mixer_group.as_deref().map_or(1.0, |g| audio.spec.mixer.gain(g));
    let full = song.volume * controller.volume * audio.spec.master_playlist_volume * mixer;
    let settings = if song.looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN };
    commands.spawn((
        Name::new(format!("music {}", song.name)),
        AudioPlayer::new(handle),
        settings.with_volume(Volume::Linear(if fading_in { 0.0 } else { full })).with_speed(song.pitch),
        MusicTrack { song: song.name.clone(), full, level: if fading_in { 0.0 } else { 1.0 }, target: 1.0, rate },
    ));
}

/// The Lobby's "Level Music" `EventSounds` start cue.
fn lobby_music(
    mut commands: Commands,
    config: Res<GameConfig>,
    audio: Option<ResMut<MasterAudio>>,
    assets: Res<AssetServer>,
    mut cache: ResMut<LobbyMusic>,
    mut tracks: Query<(Entity, &mut MusicTrack)>,
) {
    if cache.started {
        return;
    }
    let Some(mut audio) = audio else { return };
    cache.started = true;
    let cue = cache
        .cue
        .get_or_insert_with(|| {
            let assets_dir = config.assets_dir();
            let scripts = unity_data::scripts::ScriptIndex::scan(&assets_dir.join("Scripts"));
            unity_data::scene::Scene::load(&assets_dir.join("Game/Lobby/Lobby.unity")).ok().and_then(|scene| scene_sounds(&scene, &scripts).music)
        })
        .clone();
    if let Some(cue) = cue {
        start_music(&mut commands, &mut audio, &assets, &cue, &mut tracks);
    }
}

/// The level's "Level Music" `EventSounds` start cue.
fn level_music(mut commands: Commands, level: Res<ActiveLevel>, audio: Option<ResMut<MasterAudio>>, assets: Res<AssetServer>, mut tracks: Query<(Entity, &mut MusicTrack)>) {
    let Some(mut audio) = audio else { return };
    if let Some(cue) = level.0.sounds.music.clone() {
        start_music(&mut commands, &mut audio, &assets, &cue, &mut tracks);
    }
}

/// The playlist cross-fade.
fn update_music(mut commands: Commands, time: Res<Time<Real>>, mut tracks: Query<(Entity, &mut MusicTrack, Option<&mut AudioSink>)>, memory: Res<crate::lobby::LobbyMemory>) {
    // The settings page's music volume (not in the original).
    let music = memory.settings.music_volume;
    let dt = time.delta_secs();
    for (entity, mut t, sink) in &mut tracks {
        if t.level != t.target {
            let step = t.rate * dt;
            t.level = if t.level < t.target { (t.level + step).min(t.target) } else { (t.level - step).max(t.target) };
        }
        if t.level <= 0.0 && t.target <= 0.0 {
            commands.entity(entity).try_despawn();
            continue;
        }
        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(t.full * t.level * music));
        }
    }
}

/// The helicopters' engine `AudioSource`s: started while the object is active, stopped when it
/// is deactivated (asleep off screen, exploded) or gone; volume = `HelicopterControl`'s
/// `engineSound.volume` (which replaces the serialized volume) × the source's mixer group gain × its 3D rolloff at the listener, pitch
/// as `HelicopterControl.FixedUpdate` sets it.
fn vehicle_engines(
    mut commands: Commands,
    audio: Option<ResMut<MasterAudio>>,
    assets: Res<AssetServer>,
    cameras: Query<&Rig, With<LevelCamera>>,
    helicopters: Query<(Entity, &crate::helicopter::Helicopter)>,
    mut voices: Query<(Entity, &EngineVoice, Option<&mut AudioSink>)>,
    memory: Res<crate::lobby::LobbyMemory>,
) {
    let Some(mut audio) = audio else { return };
    let sound_volume = memory.settings.sound_volume;
    let ear = listener(&cameras);
    let mut sounding = Vec::new();
    for (entity, voice, sink) in &mut voices {
        let engine = helicopters.get(voice.vehicle).ok().and_then(|(_, h)| h.engine());
        let Some((sound, at, volume, pitch)) = engine else {
            commands.entity(entity).try_despawn();
            continue;
        };
        sounding.push(voice.vehicle);
        let Some(mut sink) = sink else { continue };
        let mixer = sound.mixer_group.map_or(1.0, |id| audio.spec.mixer.gain_of_id(id));
        let distance = ear.map_or(1.0, |ear| distance_gain(at, ear, sound.min_distance, sound.max_distance, sound.rolloff, sound.spatial_blend));
        sink.set_volume(Volume::Linear((volume * mixer * distance * sound_volume).max(0.0)));
        sink.set_speed(pitch);
    }
    for (vehicle, h) in &helicopters {
        if sounding.contains(&vehicle) {
            continue;
        }
        let Some((sound, _, _, pitch)) = h.engine() else { continue };
        if !sound.play_on_awake {
            continue;
        }
        let Some(path) = audio.clip_index.path(&sound.clip).map(std::path::Path::to_path_buf) else { continue };
        let handle = audio.clip(&path, &assets);
        let settings = if sound.looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN };
        commands.spawn((
            Name::new(format!("engine of {}", h.spec.name)),
            AudioPlayer::new(handle),
            // The volume is set every frame once the sink exists.
            settings.with_volume(Volume::Linear(0.0)).with_speed(pitch),
            EngineVoice { vehicle },
        ));
    }
}
