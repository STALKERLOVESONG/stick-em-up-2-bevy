//! MasterAudio as the game observes it: which voice of a sound group plays, at what volume and
//! pitch, whether it plays at all, how it fades, and how loud a 3D voice is at the listener.
//!
//! * `PlaySoundAtVolume`: groups with a time limit (`Small/Large Triumph`: 1 s) drop plays that
//!   come too soon (real time). A group with one voice always cues it; otherwise the first index
//!   of the group's randomizer list is cued. A voice that is still playing and has played less
//!   than `retriggerPercentage` of its clip refuses, and the whole play is dropped ("all
//!   children busy" — the leftover list is always empty in this game) without consuming the
//!   choice. A voice that plays is removed from the list and remembered as played; when the
//!   list runs empty it is refilled (`RefillSoundGroupPool`): the played voices, shuffled
//!   (`SortIntArray` is a Fisher–Yates shuffle), and if the most recently played one came out
//!   first it is moved to a random later place.
//! * Volume: `(clip volume [+ random]) × groupMasterVolume × bus volume × master volume ×
//!   volumePercentage`; the mixer group's gain is applied by the output. Pitch: the clip pitch,
//!   or a random one.
//! * `useFades` voices (`FadeInOut`, every frame): ramp in over `fadeInTime`; the first frame
//!   past it sets the full volume; from `length - fadeOutTime × pitch` on, the volume falls
//!   linearly to 0 at the end. `FadeOutNow` (`LifetimeSound` when its projectile goes away)
//!   steps the volume down to 0 over `(int)(fadeOutTime / deltaTime)` frames.
//! * 3D voices (`spatialBlend` 1) at the `AudioListener` (the level camera): logarithmic
//!   rolloff `minDistance / clamp(distance, minDistance, maxDistance)`.
//!
//! TODO(UNCERTAIN): `UnityEngine.Random` replaced by [`GameRng`]. Unity's stereo panning of 3D
//! voices is not reproduced (the Bevy side plays every voice centred).

use std::collections::HashMap;

use unity_compat::Vec3;
use unity_data::audio::{LimitMode, MasterAudioSpec, SoundGroup};

use crate::rng::GameRng;

/// A voice that is (or may still be) sounding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub instance: u64,
    /// `Time.realtimeSinceStartup` when it started.
    pub started: f32,
    pub pitch: f32,
    pub length: Option<f32>,
    pub looping: bool,
}

impl Playing {
    /// `AudioSource.time` at `now`.
    pub fn clip_time(&self, now: f32) -> f32 {
        (now - self.started) * self.pitch
    }

    /// `AudioSource.isPlaying`.
    pub fn is_playing(&self, now: f32) -> bool {
        self.looping || self.length.is_none_or(|l| self.clip_time(now) < l)
    }
}

#[derive(Clone, Debug, Default)]
struct GroupState {
    randomizer: Vec<usize>,
    played: Vec<usize>,
    last_time_played: Option<f32>,
    last_frame_played: Option<i64>,
    voices: Vec<Option<Playing>>,
}

/// What the output should start.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayRequest {
    pub group: usize,
    pub voice: usize,
    pub instance: u64,
    /// `AudioSource.volume` as set by `PlaySoundIfAvailable` (before the mixer and distance).
    pub volume: f32,
    pub pitch: f32,
    /// The voice instance this one cuts off (`Variation.Stop()` before replaying it).
    pub replaces: Option<u64>,
}

/// The runtime state of every sound group.
#[derive(Clone, Debug, Default)]
pub struct SoundBank {
    groups: Vec<GroupState>,
    by_name: HashMap<String, usize>,
    next_instance: u64,
}

/// `ArrayListUtil.SortIntArray` (a shuffle despite its name).
fn shuffle(list: &mut [usize], rng: &mut GameRng) {
    for i in 0..list.len() {
        let j = rng.range_int(i as i32, list.len() as i32) as usize;
        list.swap(i, j);
    }
}

impl SoundBank {
    /// `MasterAudio.Awake`: one index per voice, shuffled for randomized groups.
    pub fn new(spec: &MasterAudioSpec, rng: &mut GameRng) -> SoundBank {
        let mut bank = SoundBank::default();
        for (i, group) in spec.groups.iter().enumerate() {
            let mut randomizer: Vec<usize> = (0..group.voices.len()).collect();
            if group.randomized {
                shuffle(&mut randomizer, rng);
            }
            bank.groups.push(GroupState { randomizer, voices: vec![None; group.voices.len()], ..Default::default() });
            bank.by_name.insert(group.name.clone(), i);
        }
        bank
    }

    pub fn group_index(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// The voice currently assigned to `instance`, if it has not been replaced.
    pub fn playing(&self, group: usize, voice: usize) -> Option<Playing> {
        self.groups.get(group)?.voices.get(voice).copied().flatten()
    }

    /// `PlaySoundAtVolume` for the group named `name` at real time `now` in frame `frame`.
    pub fn play(&mut self, spec: &MasterAudioSpec, name: &str, now: f32, frame: i64, volume_percentage: f32, rng: &mut GameRng) -> Option<PlayRequest> {
        let gi = self.group_index(name)?;
        let group: &SoundGroup = &spec.groups[gi];
        let state = &mut self.groups[gi];
        if group.voices.is_empty() {
            return None;
        }
        if group.variation_mode == 0 {
            match group.limit {
                LimitMode::TimeBased(min) if min > 0.0 => {
                    if state.last_time_played.is_some_and(|last| now < last + min) {
                        return None;
                    }
                    state.last_time_played = Some(now);
                }
                LimitMode::TimeBased(_) => {}
                LimitMode::FrameBased(per) => {
                    if state.last_frame_played.is_some_and(|last| frame - last < per) {
                        return None;
                    }
                    state.last_frame_played = Some(frame);
                }
                LimitMode::None => {
                    state.last_time_played = Some(now);
                    state.last_frame_played = Some(frame);
                }
            }
            if let Some(limit) = group.voice_limit {
                let busy = state.voices.iter().flatten().filter(|p| p.is_playing(now)).count();
                if busy >= limit {
                    return None;
                }
            }
        }
        let single = group.voices.len() == 1;
        let voice = if single { 0 } else { *state.randomizer.first()? };
        let spec_voice = &group.voices[voice];
        // `PlaySoundIfAvailable`: a busy voice short of the retrigger point refuses.
        let replaces = match state.voices[voice] {
            Some(p) if p.is_playing(now) => {
                let played = match p.length {
                    Some(l) if l > 0.0 => p.clip_time(now) / l * 100.0,
                    _ => 0.0,
                };
                if group.variation_mode == 0 && played < group.retrigger_percentage {
                    return None;
                }
                Some(p.instance)
            }
            Some(p) => Some(p.instance),
            None => None,
        };
        let mut volume = spec_voice.volume;
        if let Some(r) = spec_voice.random_volume {
            let v = rng.range_f32(r.min, r.max);
            volume = if r.replaces { v } else { volume + v };
        }
        let volume = volume * group.volume * spec.bus_volume(group) * spec.master_volume * volume_percentage;
        let pitch = match spec_voice.random_pitch {
            Some(r) => {
                let p = rng.range_f32(r.min, r.max);
                if r.replaces { p } else { spec_voice.pitch + p }
            }
            None => spec_voice.pitch,
        };
        self.next_instance += 1;
        let instance = self.next_instance;
        state.voices[voice] = Some(Playing { instance, started: now, pitch, length: spec_voice.clip_length, looping: spec_voice.looping });
        if !single {
            // `RemoveClipAndRefillIfEmpty`.
            state.randomizer.remove(0);
            state.played.push(voice);
            if state.randomizer.is_empty() {
                let mut list = std::mem::take(&mut state.played);
                let last = list.last().copied();
                if group.randomized {
                    shuffle(&mut list, rng);
                    if last.is_some() && last == list.first().copied() && list.len() > 1 {
                        let item = list.remove(0);
                        let at = rng.range_int(1, list.len() as i32) as usize;
                        list.insert(at, item);
                    }
                }
                state.randomizer = list;
            }
        }
        Some(PlayRequest { group: gi, voice, instance, volume, pitch, replaces })
    }

    /// `Variation.Stop()`: the voice is free again.
    pub fn stop(&mut self, instance: u64) {
        for g in &mut self.groups {
            for v in &mut g.voices {
                if v.is_some_and(|p| p.instance == instance) {
                    *v = None;
                }
            }
        }
    }
}

/// `SoundGroupVariationUpdater` fading of one voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fade {
    None,
    /// `FadeInOut` with the volume the voice was started at.
    InOut { max: f32, fade_in: f32, fade_out_start: f32, out_factor: Option<f32>, set_max: bool },
    /// `FadeOutEarly`.
    OutEarly { frame: i32, total_frames: i32, per_frame: f32, from: f32 },
}

impl Fade {
    /// `FinishSetupToPlay` → `FadeInOut()` for a `useFades` voice.
    pub fn in_out(max: f32, fade_in: f32, fade_out: f32, length: f32, pitch: f32, looping: bool) -> Fade {
        if fade_in <= 0.0 && fade_out <= 0.0 {
            return Fade::None;
        }
        let fade_out_start = length - fade_out * pitch;
        let out_factor = (fade_out > 0.0 && !looping).then(|| max / (length - fade_out_start));
        Fade::InOut { max, fade_in, fade_out_start, out_factor, set_max: false }
    }

    /// `FadeOutEarly(fadeTime)` from the voice's current volume.
    pub fn out_early(fade_time: f32, clip_time: f32, length: Option<f32>, looping: bool, current_volume: f32, delta_time: f32) -> Fade {
        let mut t = fade_time;
        if let Some(l) = length {
            if !looping && clip_time + t > l {
                t = l - clip_time;
            }
        }
        let dt = if delta_time == 0.0 { 0.02 } else { delta_time };
        let total = (t / dt) as i32;
        Fade::OutEarly { frame: 0, total_frames: total, per_frame: (0.0 - current_volume) / total as f32, from: current_volume }
    }

    /// `PerformFading` once (per frame): the voice's volume now, if the fade sets it.
    pub fn step(&mut self, clip_time: f32, length: f32) -> Option<f32> {
        match self {
            Fade::None => None,
            Fade::InOut { max, fade_in, fade_out_start, out_factor, set_max } => {
                if *fade_in > 0.0 && clip_time < *fade_in {
                    Some(clip_time * (*max / *fade_in))
                } else if clip_time >= *fade_in && !*set_max {
                    *set_max = true;
                    let v = *max;
                    if out_factor.is_none() {
                        *self = Fade::None;
                    }
                    Some(v)
                } else if let Some(k) = *out_factor {
                    (clip_time >= *fade_out_start).then(|| (length - clip_time) * k)
                } else {
                    None
                }
            }
            Fade::OutEarly { frame, total_frames, per_frame, from } => {
                *frame += 1;
                let v = *frame as f32 * *per_frame + *from;
                if *frame >= *total_frames {
                    *self = Fade::None;
                }
                Some(v)
            }
        }
    }
}

/// `AudioSource` distance attenuation (rolloff 0 logarithmic, 1 linear) blended by
/// `spatialBlend`.
pub fn distance_gain(source: Vec3, listener: Vec3, min_distance: f32, max_distance: f32, rolloff: i64, spatial_blend: f32) -> f32 {
    let d = (source - listener).magnitude();
    let three_d = match rolloff {
        1 => {
            if d <= min_distance {
                1.0
            } else {
                (1.0 - (d - min_distance) / (max_distance - min_distance)).clamp(0.0, 1.0)
            }
        }
        _ => min_distance / d.clamp(min_distance.max(1e-6), max_distance.max(min_distance)),
    };
    1.0 + (three_d - 1.0) * spatial_blend.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_data::audio::{Mixer, SpatialBlend, Voice};

    fn voice(name: &str, length: f32) -> Voice {
        Voice {
            variation: name.into(),
            clip_guid: String::new(),
            clip: None,
            clip_length: Some(length),
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            min_distance: 10.0,
            max_distance: 500.0,
            rolloff: 0,
            spatial_blend: 1.0,
            random_pitch: None,
            random_volume: None,
            fades: None,
        }
    }

    fn spec(voices: Vec<Voice>, limit: LimitMode) -> MasterAudioSpec {
        MasterAudioSpec {
            groups: vec![SoundGroup {
                name: "G".into(),
                bus: None,
                spatial: SpatialBlend::ThreeD,
                volume: 0.5,
                retrigger_percentage: 50.0,
                limit,
                voice_limit: None,
                randomized: true,
                variation_mode: 0,
                voices,
            }],
            buses: Vec::new(),
            playlists: Vec::new(),
            playlist_controller: None,
            master_volume: 1.0,
            master_playlist_volume: 1.0,
            mixer_spatial: SpatialBlend::FromSource,
            music_spatial: SpatialBlend::TwoD,
            mixer: Mixer::default(),
        }
    }

    #[test]
    fn every_voice_plays_once_before_any_repeats_and_no_voice_twice_in_a_row() {
        let s = spec((0..6).map(|i| voice(&format!("v{i}"), 0.1)).collect(), LimitMode::None);
        let mut rng = GameRng::from_seed(11);
        let mut bank = SoundBank::new(&s, &mut rng);
        let mut order = Vec::new();
        let mut now = 0.0;
        for frame in 0..60 {
            now += 1.0;
            let r = bank.play(&s, "G", now, frame, 1.0, &mut rng).expect("idle voices always play");
            assert_eq!(r.volume, 0.5, "clip volume × group volume");
            order.push(r.voice);
        }
        for round in order.chunks(6) {
            let mut sorted = round.to_vec();
            sorted.sort();
            assert_eq!(sorted, (0..6).collect::<Vec<_>>());
        }
        assert!(order.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn a_busy_voice_short_of_the_retrigger_point_drops_the_play_and_keeps_its_turn() {
        let s = spec(vec![voice("a", 1.0), voice("b", 1.0)], LimitMode::None);
        let mut rng = GameRng::from_seed(3);
        let mut bank = SoundBank::new(&s, &mut rng);
        let first = bank.play(&s, "G", 0.0, 0, 1.0, &mut rng).unwrap();
        let second = bank.play(&s, "G", 0.0, 0, 1.0, &mut rng).unwrap();
        assert_ne!(first.voice, second.voice);
        // Refilled with both played; whichever comes first is busy at 30 % played.
        assert!(bank.play(&s, "G", 0.3, 1, 1.0, &mut rng).is_none());
        assert!(bank.play(&s, "G", 0.3, 1, 1.0, &mut rng).is_none(), "the choice was not consumed");
        // Past 50 % it may be cut off and restarted.
        let third = bank.play(&s, "G", 0.6, 2, 1.0, &mut rng).unwrap();
        assert!(third.replaces.is_some());
    }

    #[test]
    fn time_limited_groups_drop_plays_within_their_window() {
        let s = spec(vec![voice("a", 0.1)], LimitMode::TimeBased(1.0));
        let mut rng = GameRng::from_seed(1);
        let mut bank = SoundBank::new(&s, &mut rng);
        assert!(bank.play(&s, "G", 5.0, 0, 1.0, &mut rng).is_some());
        assert!(bank.play(&s, "G", 5.9, 1, 1.0, &mut rng).is_none());
        assert!(bank.play(&s, "G", 6.0, 2, 1.0, &mut rng).is_some());
    }

    #[test]
    fn fade_in_out_ramps_and_fades_out_at_the_end() {
        // RPG voices: no fade in, 0.85 s fade out.
        let mut f = Fade::in_out(1.0, 0.0, 0.85, 2.0, 1.0, false);
        assert_eq!(f.step(0.01, 2.0), Some(1.0), "the first frame sets the full volume");
        assert_eq!(f.step(0.5, 2.0), None);
        let v = f.step(1.575, 2.0).unwrap();
        assert!((v - 0.5).abs() < 1e-5, "{v}");
        assert!(f.step(2.0, 2.0).unwrap().abs() < 1e-6);
        let mut early = Fade::out_early(0.85, 0.2, Some(2.0), false, 0.8, 0.1);
        let vols: Vec<f32> = (0..8).filter_map(|_| early.step(0.0, 2.0)).collect();
        assert_eq!(vols.len(), 8);
        assert!((vols[0] - 0.7).abs() < 1e-5 && vols[7].abs() < 1e-5);
        assert_eq!(early, Fade::None);
    }

    #[test]
    fn logarithmic_rolloff_and_blend() {
        let at = Vec3::new(0.0, 0.0, 0.0);
        assert_eq!(distance_gain(at, Vec3::new(0.0, 0.0, -5.0), 10.0, 500.0, 0, 1.0), 1.0);
        assert!((distance_gain(at, Vec3::new(0.0, 0.0, -40.0), 10.0, 500.0, 0, 1.0) - 0.25).abs() < 1e-6);
        assert!((distance_gain(at, Vec3::new(0.0, 0.0, -1000.0), 10.0, 500.0, 0, 1.0) - 0.02).abs() < 1e-6);
        assert_eq!(distance_gain(at, Vec3::new(0.0, 0.0, -40.0), 10.0, 500.0, 0, 0.0), 1.0, "2D voices do not attenuate");
    }
}
