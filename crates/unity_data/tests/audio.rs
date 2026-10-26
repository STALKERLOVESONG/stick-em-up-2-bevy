//! MasterAudio groups, buses, music and mixer gains read from `Permanent System.unity`.

use std::path::PathBuf;

use unity_data::audio::{LimitMode, SpatialBlend, clip_length, load_master_audio, scene_sounds};
use unity_data::scene::Scene;
use unity_data::scripts::ScriptIndex;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn every_sound_group_the_scripts_name_exists_with_its_voices() {
    let Some(root) = export_root() else { return };
    let audio = load_master_audio(&root.join("Assets")).unwrap();
    assert_eq!(audio.groups.len(), 14);
    for name in ["Assault Rifle SG 552", "Rocket-Propelled Grenade Launcher RPG-7", "Ricochet", "Small Explosion", "Paper Slide", "Select", "Back", "Applause"] {
        assert!(audio.group(name).is_some(), "{name}");
    }
    let rifle = audio.group("Assault Rifle SG 552").unwrap();
    // 8 variations × weight 3.
    assert_eq!(rifle.voices.len(), 24);
    assert!(rifle.randomized);
    assert_eq!(rifle.retrigger_percentage, 50.0);
    assert_eq!(rifle.spatial, SpatialBlend::ThreeD);
    assert_eq!(audio.buses[rifle.bus.unwrap()].name, "Assault Rifle SG 552");
    assert!(rifle.voices.iter().all(|v| v.min_distance == 10.0 && v.max_distance == 500.0 && v.rolloff == 0));
    // Weight clones sit next to each other.
    assert!(rifle.voices.chunks(3).all(|c| c.iter().all(|v| v.variation == c[0].variation)));

    let rpg = audio.group("Rocket-Propelled Grenade Launcher RPG-7").unwrap();
    assert_eq!(rpg.voices.len(), 5 + 5 + 5 + 1 + 5);
    assert!(rpg.voices.iter().all(|v| v.fades == Some((0.0, 0.85))));
    let triumph = audio.group("Small Triumph").unwrap();
    assert_eq!(triumph.limit, LimitMode::TimeBased(1.0));
    assert_eq!(triumph.spatial, SpatialBlend::TwoD);
    let hmg = audio.group("Heavy Machine Gun NSV").unwrap();
    let random_volume = hmg.voices[0].random_volume.unwrap();
    assert!(!random_volume.replaces && random_volume.min == -0.29 && random_volume.max == 0.0);
    assert_eq!(audio.mixer_spatial, SpatialBlend::FromSource, "AllowDifferentPerGroup");
}

#[test]
fn the_mixer_attenuates_each_bus_through_its_parents() {
    let Some(root) = export_root() else { return };
    let audio = load_master_audio(&root.join("Assets")).unwrap();
    let rifle = audio.group("Assault Rifle SG 552").unwrap();
    // Assault Rifle -4.94 dB, Weapons -5.8 dB, World +8.61 dB, Sound Effects and Master 0 dB.
    let expected = 10f32.powf((-4.935_643_7 - 5.8 + 8.614_072) / 20.0);
    assert!((audio.mixer_gain(rifle) - expected).abs() < 1e-4, "{}", audio.mixer_gain(rifle));
    let heli = audio.mixer.gain("Helicopter");
    assert!((heli - 10f32.powf((18.232_256 + 8.614_072) / 20.0)).abs() < 1e-3);
    let music = audio.playlist_controller.as_ref().unwrap();
    assert_eq!(music.name, "Music");
    assert_eq!(music.start_playlist, "Music");
    assert!(!music.start_on_awake);
    assert_eq!(music.mixer_group.as_deref(), Some("Music"));
    let playlist = &audio.playlists[0];
    assert_eq!(playlist.name, "Music");
    assert_eq!(playlist.crossfade_time, 0.5);
    let road_runner = playlist.songs.iter().find(|s| s.name == "Road Runner").unwrap();
    assert!(road_runner.looping);
    assert_eq!(road_runner.clip.as_deref(), Some(std::path::Path::new("AudioClip/Road Runner.ogg")));
}

#[test]
fn levels_cue_their_music_on_start() {
    let Some(root) = export_root() else { return };
    let assets = root.join("Assets");
    let scripts = ScriptIndex::scan(&assets.join("Scripts"));
    let scene = Scene::load(&assets.join("Game/Levels/Platforms/Platforms.unity")).unwrap();
    let sounds = scene_sounds(&scene, &scripts);
    let music = sounds.music.expect("Level Music");
    assert_eq!((music.playlist_controller.as_str(), music.song.as_str()), ("Music", "Road Runner"));
    assert!(sounds.start_sounds.contains(&("Ender".to_owned(), "Applause".to_owned())), "the level ender's applause");
}

#[test]
fn clip_lengths_come_from_the_file_headers() {
    let Some(root) = export_root() else { return };
    let clips = root.join("Assets/AudioClip");
    let wav = clip_length(&clips.join("GunMachineGun_S08WA.734_01.wav")).unwrap();
    assert!(wav > 0.05 && wav < 1.0, "{wav}");
    let ogg = clip_length(&clips.join("ricochet 1.ogg")).unwrap();
    assert!(ogg > 0.05 && ogg < 3.0, "{ogg}");
}
