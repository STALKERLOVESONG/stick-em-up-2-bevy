//! Audio data: the MasterAudio set-up in `Permanent System.unity` (sound groups, their voices,
//! buses, the music playlist), the Unity `AudioMixer` it routes into (`Mixer.mixer`), each
//! level's music cue (`EventSounds` on "Level Music") and the length of every clip.
//!
//! What the game observably does with it lives in `sticks_sim::audio`; this module only reads.
//!
//! * `MasterAudio.Awake` turns every child of the MasterAudio object into a sound group; each
//!   `SoundGroupVariation` child becomes `weight` voices (the original plus `weight - 1` clones,
//!   kept next to each other), each an `AudioSource` with the variation's clip and settings.
//! * A group's `busIndex` is 0 (none), 1 (new bus) or `2 + index` into `groupBuses`; a bus
//!   routes its voices to `mixerChannel`, an `AudioMixerGroup` of `Mixer.mixer`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::assets::GuidIndex;
use crate::scene::{GameObject, Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// `AudioSource` class id.
pub const AUDIO_SOURCE: u32 = 82;

/// `MasterAudio.ItemSpatialBlendType` / `AllMixerSpatialBlendType`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpatialBlend {
    /// `ForceTo2D`.
    TwoD,
    /// `ForceTo3D`.
    ThreeD,
    /// `ForceToCustom`: the blend value.
    Custom(f32),
    /// `UseCurveFromAudioSource` / `AllowDifferentPerGroup`: the source's own setting.
    FromSource,
}

impl SpatialBlend {
    fn from(kind: i64, value: f32) -> SpatialBlend {
        match kind {
            0 => SpatialBlend::TwoD,
            1 => SpatialBlend::ThreeD,
            2 => SpatialBlend::Custom(value),
            _ => SpatialBlend::FromSource,
        }
    }

    /// The blend (0 = 2D, 1 = 3D) with `source` as the fallback.
    pub fn value(self, source: f32) -> f32 {
        match self {
            SpatialBlend::TwoD => 0.0,
            SpatialBlend::ThreeD => 1.0,
            SpatialBlend::Custom(v) => v,
            SpatialBlend::FromSource => source,
        }
    }
}

/// `SoundGroupVariation.randomPitchMode` / `randomVolumeMode`: 0 adds to the clip's value,
/// 1 replaces it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RandomRange {
    pub replaces: bool,
    pub min: f32,
    pub max: f32,
}

/// One voice of a group: a variation's `AudioSource` (or one of its weight clones).
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// The variation object's name.
    pub variation: String,
    pub clip_guid: String,
    /// The clip file, relative to `Assets/` (`None` if the clip is not in the export).
    pub clip: Option<PathBuf>,
    /// Clip length in seconds (`AudioClip.length`), when it could be read.
    pub clip_length: Option<f32>,
    /// `AudioSource.volume` / `pitch` / `loop` as authored (the `OriginalVolume`).
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
    pub min_distance: f32,
    pub max_distance: f32,
    /// `AudioSource.rolloffMode`: 0 logarithmic, 1 linear, 2 custom.
    pub rolloff: i64,
    pub spatial_blend: f32,
    pub random_pitch: Option<RandomRange>,
    pub random_volume: Option<RandomRange>,
    /// `useFades`: `(fadeInTime, fadeOutTime)`.
    pub fades: Option<(f32, f32)>,
}

/// `MasterAudioGroup.limitMode`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LimitMode {
    None,
    /// At most one play every `limitPerXFrames` frames.
    FrameBased(i64),
    /// At most one play every `minimumTimeBetween` seconds (real time).
    TimeBased(f32),
}

/// A MasterAudio sound group (`MasterAudioGroup`).
#[derive(Clone, Debug, PartialEq)]
pub struct SoundGroup {
    pub name: String,
    pub bus: Option<usize>,
    pub spatial: SpatialBlend,
    pub volume: f32,
    /// `retriggerPercentage`: a busy voice may be restarted once it has played this much.
    pub retrigger_percentage: f32,
    pub limit: LimitMode,
    /// `limitPolyphony` + `voiceLimitCount`.
    pub voice_limit: Option<usize>,
    /// `curVariationSequence`: true for `Randomized` (0), false for `TopToBottom` (1).
    pub randomized: bool,
    /// `curVariationMode`: 0 normal, 1 looped chain, 2 dialog.
    pub variation_mode: i64,
    pub voices: Vec<Voice>,
}

/// `GroupBus`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bus {
    pub name: String,
    pub volume: f32,
    /// `voiceLimit` (-1: none) and `stopOldest`.
    pub voice_limit: Option<usize>,
    pub stop_oldest: bool,
    pub mixer_group: Option<String>,
}

/// `MusicSetting`.
#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub name: String,
    pub clip: Option<PathBuf>,
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
}

/// `Playlist`.
#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub name: String,
    pub songs: Vec<Song>,
    /// `crossfadeMode` 0 uses the MasterAudio's `crossFadeTime`, 1 this playlist's.
    pub crossfade_time: f32,
}

/// The `PlaylistController` named "Music".
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistControllerSpec {
    pub name: String,
    pub start_playlist: String,
    pub start_on_awake: bool,
    pub volume: f32,
    pub mixer_group: Option<String>,
}

/// One `AudioMixerGroup` with its snapshot volume.
#[derive(Clone, Debug, PartialEq)]
pub struct MixerGroup {
    /// The group's file id in `Mixer.mixer` (what an `AudioSource.OutputAudioMixerGroup` names).
    pub file_id: i64,
    pub name: String,
    pub volume_db: f32,
    pub parent: Option<usize>,
}

/// `Mixer.mixer`'s start snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mixer {
    pub groups: Vec<MixerGroup>,
}

impl Mixer {
    /// Linear gain of everything routed to `group` (its volume times every ancestor's).
    pub fn gain(&self, group: &str) -> f32 {
        let Some(mut i) = self.groups.iter().position(|g| g.name == group) else { return 1.0 };
        let mut db = 0.0;
        loop {
            db += self.groups[i].volume_db;
            match self.groups[i].parent {
                Some(p) => i = p,
                None => break,
            }
        }
        10f32.powf(db / 20.0)
    }

    /// [`Mixer::gain`] of the group with this file id (unity gain when unknown).
    pub fn gain_of_id(&self, file_id: i64) -> f32 {
        self.groups.iter().find(|g| g.file_id == file_id).map_or(1.0, |g| self.gain(&g.name))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MasterAudioSpec {
    pub groups: Vec<SoundGroup>,
    pub buses: Vec<Bus>,
    pub playlists: Vec<Playlist>,
    pub playlist_controller: Option<PlaylistControllerSpec>,
    pub master_volume: f32,
    pub master_playlist_volume: f32,
    /// `mixerSpatialBlendType` / `mixerSpatialBlend`.
    pub mixer_spatial: SpatialBlend,
    /// `musicSpatialBlendType` / `musicSpatialBlend`.
    pub music_spatial: SpatialBlend,
    pub mixer: Mixer,
}

impl MasterAudioSpec {
    pub fn group(&self, name: &str) -> Option<&SoundGroup> {
        self.groups.iter().find(|g| g.name == name)
    }

    /// `GetBusVolume`.
    pub fn bus_volume(&self, group: &SoundGroup) -> f32 {
        group.bus.and_then(|b| self.buses.get(b)).map_or(1.0, |b| b.volume)
    }

    /// Mixer gain of a group's output (its bus's channel).
    pub fn mixer_gain(&self, group: &SoundGroup) -> f32 {
        group.bus.and_then(|b| self.buses.get(b)).and_then(|b| b.mixer_group.as_deref()).map_or(1.0, |g| self.mixer.gain(g))
    }

    /// The spatial blend a group's voices end up with (`MasterAudio.Awake` applies the mixer's
    /// setting unless it allows per-group settings).
    pub fn spatial_blend(&self, group: &SoundGroup, voice: &Voice) -> f32 {
        match self.mixer_spatial {
            SpatialBlend::FromSource => group.spatial.value(voice.spatial_blend),
            other => other.value(voice.spatial_blend),
        }
    }
}

fn find_script<'a>(go: GameObject<'a>, scripts: &ScriptIndex, class_name: &str) -> Option<&'a Value> {
    go.components().into_iter().find_map(|c| {
        let guid = c.body.get("m_Script").and_then(Value::guid)?;
        (c.class_id == class::MONO_BEHAVIOUR && scripts.class_name(guid) == Some(class_name)).then_some(&c.body)
    })
}

/// Clip files under `Assets/AudioClip` by GUID (relative to `Assets/`).
pub struct ClipIndex {
    by_guid: HashMap<String, PathBuf>,
    assets_dir: PathBuf,
}

impl ClipIndex {
    pub fn scan(assets_dir: &Path) -> ClipIndex {
        let mut by_guid = HashMap::new();
        for ext in ["ogg", "wav", "mp3", "aif", "aiff"] {
            let index = GuidIndex::scan(&assets_dir.join("AudioClip"), ext);
            for guid in index.guids() {
                if let Some(path) = index.path(&guid) {
                    let relative = path.strip_prefix(assets_dir).unwrap_or(path).to_path_buf();
                    by_guid.insert(guid, relative);
                }
            }
        }
        ClipIndex { by_guid, assets_dir: assets_dir.to_path_buf() }
    }

    pub fn path(&self, guid: &str) -> Option<&Path> {
        self.by_guid.get(guid).map(PathBuf::as_path)
    }

    /// `AudioClip.length` read from the file header.
    pub fn length(&self, guid: &str) -> Option<f32> {
        clip_length(&self.assets_dir.join(self.path(guid)?))
    }
}

/// Length in seconds of a WAV (RIFF PCM) or Ogg Vorbis file.
pub fn clip_length(path: &Path) -> Option<f32> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(b"RIFF") {
        wav_length(&bytes)
    } else if bytes.starts_with(b"OggS") {
        ogg_vorbis_length(&bytes)
    } else {
        None
    }
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn wav_length(b: &[u8]) -> Option<f32> {
    let mut i = 12;
    let (mut byte_rate, mut data) = (None, None);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let size = u32_at(b, i + 4)? as usize;
        if id == b"fmt " {
            byte_rate = u32_at(b, i + 8 + 8);
        } else if id == b"data" {
            data = Some(size);
        }
        i += 8 + size + (size & 1);
    }
    Some(data? as f32 / byte_rate? as f32)
}

/// Sample rate from the Vorbis identification header, sample count from the last page's
/// granule position.
fn ogg_vorbis_length(b: &[u8]) -> Option<f32> {
    // First page: header (27 + segment table), then the identification packet:
    // 0x01 "vorbis" version(4) channels(1) rate(4).
    let segments = *b.get(26)? as usize;
    let packet = 27 + segments;
    if b.get(packet..packet + 7)? != b"\x01vorbis" {
        return None;
    }
    let rate = u32_at(b, packet + 12)?;
    let last = b.windows(4).rposition(|w| w == b"OggS")?;
    let granule = u64::from_le_bytes(b.get(last + 6..last + 14)?.try_into().ok()?);
    (rate > 0).then(|| granule as f32 / rate as f32)
}

fn file_id_of(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::file_id).filter(|&f| f != 0)
}

/// Read `Mixer.mixer` (groups, their parents and the start snapshot's volumes).
pub fn parse_mixer(text: &str) -> (Mixer, HashMap<i64, usize>) {
    let scene = Scene::parse(text);
    let groups: Vec<&crate::yaml::Document> = scene.documents().filter(|d| d.class_id == 243).collect();
    let snapshot = scene.documents().find(|d| d.class_id == 245);
    let values = snapshot.and_then(|s| s.body.get("m_FloatValues"));
    let mut index_of: HashMap<i64, usize> = HashMap::new();
    for (i, g) in groups.iter().enumerate() {
        index_of.insert(g.file_id, i);
    }
    let mut mixer = Mixer::default();
    for g in &groups {
        let volume_param = g.body.str_of("m_Volume").unwrap_or("");
        let volume_db = values.and_then(|v| v.f32_of(volume_param)).unwrap_or(0.0);
        mixer.groups.push(MixerGroup { file_id: g.file_id, name: g.body.str_of("m_Name").unwrap_or("").to_owned(), volume_db, parent: None });
    }
    for g in &groups {
        let parent = index_of[&g.file_id];
        for child in g.body.get("m_Children").map(Value::as_seq).unwrap_or_default() {
            if let Some(&c) = child.file_id().and_then(|f| index_of.get(&f)) {
                mixer.groups[c].parent = Some(parent);
            }
        }
    }
    (mixer, index_of)
}

fn random_range(b: &Value, prefix: &str) -> Option<RandomRange> {
    b.bool_of(&format!("useRandom{prefix}")).unwrap_or(false).then(|| RandomRange {
        replaces: b.i64_of(&format!("random{prefix}Mode")).unwrap_or(0) == 1,
        min: b.f32_of(&format!("random{prefix}Min")).unwrap_or(0.0),
        max: b.f32_of(&format!("random{prefix}Max")).unwrap_or(0.0),
    })
}

/// Parse the MasterAudio of `Permanent System.unity` and the mixer it uses.
pub fn parse_master_audio(scene: &Scene, scripts: &ScriptIndex, clips: &ClipIndex, mixer_text: Option<&str>) -> Option<MasterAudioSpec> {
    let objects = scene.game_objects();
    let (master_go, master) = objects.iter().find_map(|go| find_script(*go, scripts, "MasterAudio").map(|b| (*go, b)))?;
    let (mixer, mixer_ids) = mixer_text.map(parse_mixer).unwrap_or_default();
    let mixer_name = |v: Option<&Value>| file_id_of(v).and_then(|f| mixer_ids.get(&f)).map(|&i| mixer.groups[i].name.clone());

    let buses = master
        .get("groupBuses")
        .map(Value::as_seq)
        .unwrap_or_default()
        .iter()
        .map(|b| Bus {
            name: b.str_of("busName").unwrap_or("").to_owned(),
            volume: b.f32_of("volume").unwrap_or(1.0),
            voice_limit: b.i64_of("voiceLimit").filter(|&l| l >= 0).map(|l| l as usize),
            stop_oldest: b.bool_of("stopOldest").unwrap_or(false),
            mixer_group: mixer_name(b.get("mixerChannel")),
        })
        .collect();

    let master_crossfade = master.f32_of("crossFadeTime").unwrap_or(1.0);
    let playlists = master
        .get("musicPlaylists")
        .map(Value::as_seq)
        .unwrap_or_default()
        .iter()
        .map(|p| Playlist {
            name: p.str_of("playlistName").unwrap_or("").to_owned(),
            songs: p
                .get("MusicSettings")
                .map(Value::as_seq)
                .unwrap_or_default()
                .iter()
                .map(|s| Song {
                    name: s.str_of("songName").unwrap_or("").to_owned(),
                    clip: s.get("clip").and_then(Value::guid).and_then(|g| clips.path(g)).map(Path::to_path_buf),
                    volume: s.f32_of("volume").unwrap_or(1.0),
                    pitch: s.f32_of("pitch").unwrap_or(1.0),
                    looping: s.bool_of("isLoop").unwrap_or(false),
                })
                .collect(),
            crossfade_time: if p.i64_of("crossfadeMode").unwrap_or(0) == 1 { p.f32_of("crossFadeTime").unwrap_or(1.0) } else { master_crossfade },
        })
        .collect();

    let mut groups = Vec::new();
    for child in master_go.children() {
        let Some(g) = find_script(child, scripts, "MasterAudioGroup") else { continue };
        let bus_index = g.i64_of("busIndex").unwrap_or(0);
        let limit = match g.i64_of("limitMode").unwrap_or(0) {
            1 => LimitMode::FrameBased(g.i64_of("limitPerXFrames").unwrap_or(1)),
            2 => LimitMode::TimeBased(g.f32_of("minimumTimeBetween").unwrap_or(0.1)),
            _ => LimitMode::None,
        };
        let mut voices = Vec::new();
        for variation_go in child.children() {
            let Some(v) = find_script(variation_go, scripts, "SoundGroupVariation") else { continue };
            let Some(source) = variation_go.component(AUDIO_SOURCE) else { continue };
            let s = &source.body;
            let clip_guid = s.get("m_audioClip").and_then(Value::guid).unwrap_or("").to_owned();
            let voice = Voice {
                variation: variation_go.name().to_owned(),
                clip: clips.path(&clip_guid).map(Path::to_path_buf),
                clip_length: clips.length(&clip_guid),
                clip_guid,
                volume: s.f32_of("m_Volume").unwrap_or(1.0),
                pitch: s.f32_of("m_Pitch").unwrap_or(1.0),
                looping: s.bool_of("Loop").unwrap_or(false),
                min_distance: s.f32_of("MinDistance").unwrap_or(1.0),
                max_distance: s.f32_of("MaxDistance").unwrap_or(500.0),
                rolloff: s.i64_of("rolloffMode").unwrap_or(0),
                spatial_blend: 0.0,
                random_pitch: random_range(v, "Pitch"),
                random_volume: random_range(v, "Volume"),
                fades: v.bool_of("useFades").unwrap_or(false).then(|| (v.f32_of("fadeInTime").unwrap_or(0.0), v.f32_of("fadeOutTime").unwrap_or(0.0))),
            };
            let weight = v.i64_of("weight").unwrap_or(1).max(0) as usize;
            for _ in 0..weight {
                voices.push(voice.clone());
            }
        }
        groups.push(SoundGroup {
            name: child.name().to_owned(),
            bus: (bus_index >= 2).then(|| (bus_index - 2) as usize),
            spatial: SpatialBlend::from(g.i64_of("spatialBlendType").unwrap_or(1), g.f32_of("spatialBlend").unwrap_or(1.0)),
            volume: g.f32_of("groupMasterVolume").unwrap_or(1.0),
            retrigger_percentage: g.f32_of("retriggerPercentage").unwrap_or(50.0),
            limit,
            voice_limit: g.bool_of("limitPolyphony").unwrap_or(false).then(|| g.i64_of("voiceLimitCount").unwrap_or(1).max(0) as usize),
            randomized: g.i64_of("curVariationSequence").unwrap_or(0) == 0,
            variation_mode: g.i64_of("curVariationMode").unwrap_or(0),
            voices,
        });
    }

    let playlist_controller = objects.iter().find_map(|go| {
        let b = find_script(*go, scripts, "PlaylistController")?;
        Some(PlaylistControllerSpec {
            name: go.name().to_owned(),
            start_playlist: b.str_of("startPlaylistName").unwrap_or("").to_owned(),
            start_on_awake: b.bool_of("startPlaylistOnAwake").unwrap_or(false),
            volume: b.f32_of("_playlistVolume").unwrap_or(1.0),
            mixer_group: mixer_name(b.get("mixerChannel")),
        })
    });

    Some(MasterAudioSpec {
        groups,
        buses,
        playlists,
        playlist_controller,
        master_volume: master.f32_of("_masterAudioVolume").unwrap_or(1.0),
        master_playlist_volume: master.f32_of("_masterPlaylistVolume").unwrap_or(1.0),
        mixer_spatial: SpatialBlend::from(master.i64_of("mixerSpatialBlendType").unwrap_or(3), master.f32_of("mixerSpatialBlend").unwrap_or(1.0)),
        music_spatial: SpatialBlend::from(master.i64_of("musicSpatialBlendType").unwrap_or(0), master.f32_of("musicSpatialBlend").unwrap_or(0.0)),
        mixer,
    })
}

/// Load the MasterAudio set-up from an export's `Assets/` directory.
pub fn load_master_audio(assets_dir: &Path) -> std::io::Result<MasterAudioSpec> {
    let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
    let clips = ClipIndex::scan(assets_dir);
    let scene = Scene::load(&assets_dir.join("Game/Permanent System.unity"))?;
    let mixer = std::fs::read_to_string(assets_dir.join("AudioMixerController/Mixer.mixer")).ok();
    parse_master_audio(&scene, &scripts, &clips, mixer.as_deref()).ok_or_else(|| std::io::Error::other("Permanent System has no MasterAudio"))
}

/// What an `EventSounds` start event does with the music (`PlaylistControl` → `PlayClip`).
#[derive(Clone, Debug, PartialEq)]
pub struct MusicCue {
    pub playlist_controller: String,
    pub song: String,
}

/// Every `EventSounds.startSound` action of a scene: `(object name, sound group)` for
/// `PlaySound`, and the music cue of a `PlaylistControl`/`PlayClip`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneSounds {
    pub start_sounds: Vec<(String, String)>,
    pub music: Option<MusicCue>,
}

pub fn scene_sounds(scene: &Scene, scripts: &ScriptIndex) -> SceneSounds {
    let mut out = SceneSounds::default();
    for go in scene.game_objects() {
        let Some(b) = find_script(go, scripts, "EventSounds") else { continue };
        let events = b.get("startSound").and_then(|s| s.get("SoundEvents")).map(Value::as_seq).unwrap_or_default();
        for e in events {
            match e.i64_of("currentSoundFunctionType").unwrap_or(0) {
                0 => {
                    if let Some(group) = e.str_of("soundType").filter(|s| !s.is_empty() && *s != "[None]") {
                        out.start_sounds.push((go.name().to_owned(), group.to_owned()));
                    }
                }
                3 if e.i64_of("currentPlaylistCommand") == Some(3) => {
                    out.music = Some(MusicCue {
                        playlist_controller: e.str_of("playlistControllerName").unwrap_or("").to_owned(),
                        song: e.str_of("clipName").unwrap_or("").to_owned(),
                    });
                }
                _ => {}
            }
        }
    }
    out
}
