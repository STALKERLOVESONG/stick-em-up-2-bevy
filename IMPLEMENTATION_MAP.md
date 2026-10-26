# Stick 'Em Up 2 — Bevy reimplementation: implementation map

State on 2026-10-26 (after the effects, audio, vehicle-boarding and lobby-slot passes, on top of the rendering, lobby, level-art, HUD, explosion, helicopter H1–H3 and gameplay passes). `progress.html` at the project root is the visual audit of these sections. Code lives only in `reconstruction/` (AGENTS.md makes every other folder read-only). Reference build: Steam, Unity 5.1.3f1, Mono. Session-by-session details: `reconstruction/.claude-handoff/<timestamp>/`.

## 0. Rule used to reconcile the two instruction sets

Structure is Bevy-native (components, resources, messages, plugins, states, system sets); observable behaviour is Unity-exact (update order, f32 operator order, fixed-step policy, recovered constants). Where idiomatic Bevy and fidelity disagree, fidelity wins and the choice is recorded here. No fake Unity engine: compatibility code only where it reproduces observable behaviour.

## 1. Layout (cargo workspace in `reconstruction/crates/`)

| Crate | Bevy | Role |
| --- | --- | --- |
| `unity_compat` | no | Unity numerics: `Mathf` (incl. `Lerp`/`Clamp01`), `SmoothDamp`, fixed-step/time policy, `Vec2/Vec3/Quat` |
| `unity_data` | no | Readers for the exported project: YAML (with double-quote escapes), **MasterAudio set-up + `Mixer.mixer` (`audio`)**, **Shuriken particle prefabs (`particles`)**, **helicopters incl. control slot, camera influencer, target, engine `AudioSource` (`vehicles`)**, scenes → levels (colliders incl. disabled ones that a `PolygonColliderRenderer` still draws, surfaces with `maximumTriangleArea`, lights, camera chain), interactive scenes (object tree, scripts with fields, renderers under inactive objects; local TRS), tk2d sprite collections (texel size, bounds, flip mode, pixels per metre, PNG-bytes atlases) and scene sprites (`tk2dSprite`, `tk2dTiledSprite`, `tk2dClippedSprite`, `FakeParallax`, `ScaleSpriteWithCameraDistance`, shadows), tk2d fonts/texts, clips, animator controller, Physics2D settings, materials, humanoid avatar, `Man` rig + weapons, projectile prefabs |
| `sticks_sim` | no | Rules and geometry: collision world, character controller, animator, pose solver, camera rig, life/death/respawn, stick-figure geometry (`visual`), aim + IK, weapons, enemy AI, **draw queue + vertex lighting (`draw`)**, **`PolygonColliderRenderer` mesh (`surface`)**, **tk2d text (`text`)**, **explosions incl. sticking (`explosion`)**, **particles (`particles`)**, **MasterAudio voices/fades/rolloff (`audio`)**, **`ControlSlot` boarding rules (`vehicles`)**, **lobby teams / spawn-slot / default colours (`lobby`)**, helicopters (`helicopter`), projectiles (`weapons`) |
| `sticks_game` | **0.19** | App: states, plugins, devices, scheduling, rendering. Modules: `level`, `player`, `camera`, `life`, `figure`, `weapon`, `enemy`, `helicopter`, `vehicles` (`VehiclePlugin`), `effects` (`EffectsPlugin`), `audio` (`GameAudioPlugin`), `hud`, `lobby` (`LobbyPlugin`), `unity_render`, `menu`, `debug`, `devscript`, `state`, `config` |

The old `reconstruction/src` and a stale root `Cargo.lock` must be deleted by hand (not part of the build).

## 2. Frame order (mirrors Unity)

`Input` (clock) → `FixedSteps` (0..n × [character `FixedUpdate` + helicopter flight + physics], then projectile steps (contacts, sticking anchors), projectile explosions, Abyss triggers, damage) → `RoleInput` (`CharacterControl2D.Start`; `CharacterInput.Update` / `HelicopterInput.Update`, execution order −100) → `Characters` (explosion start (`Explosion.Go`), death timers, explosion `Update`, vehicle boarding → exit → disembark, helicopter sleep/think/fire, enemy sleep + behaviours, `Update`, animator, weapon systems, projectile spawn/pool/expiry) → `Camera` → `Present` (transforms, HUD scripts) → `QueueSet::Light` (camera-borne light, `FixWorldZ`) → `QueueSet::Collect` (figures, projectiles, explosions, helicopters, particles, lobby) → `QueueSet::Draw` (particle simulation — after `LateUpdate`, so drawn one frame late as in the original — level statics + moving sprites + shadows, sort, upload; HUD queue). Audio runs in `Update` (MasterAudio voices, music cross-fade, engine loops).

**Input timing (evidence: `CharacterInput.cs.meta` executionOrder −100; `CharacterControl2D.FixedUpdate` reads `input.Jump`/`input.Movement`):** fixed steps act on what `CharacterInput` copied during the previous frame's `Update`. `FixedUpdateSynchronizer` retunes `fixedDeltaTime` to the frame time after every step.

## 3. Rendering (how the original draws, and how it is reproduced)

| Topic | Evidence | Implementation |
| --- | --- | --- |
| Rendering path | `ProjectSettings m_RenderingPath: 0`, cameras −1 → legacy **Vertex Lit**; only the `LOD 100` `LightMode=Vertex` pass of "Sprite"/"Textured Polygon"/"Color Polygon" runs (`assets_raw/Assets/Shader/*.bytes`) | `draw::vertex_lit` on the CPU (`ColorMaterial AmbientAndDiffuse`, point light `1/(1+25 d²/r²)`, range cut-off), shader `unity_lit.wgsl` = `clamp(tex × tint × vc × 2)`, alpha `tex.a × tint.a × vc.a`, `Blend SrcAlpha OneMinusSrcAlpha`, ZTest Always, ZWrite Off, Cull Back with Unity's clockwise fronts |
| Light intensity | Steam screenshot `ss_32e51c52…` (enemy lines 63/255) | `FIXED_FUNCTION_LIGHT_SCALE` 0.5 on ambient and light colour (TODO(UNCERTAIN)) |
| Gamma colour space | `m_ActiveColorSpace: 0` | colours converted to linear in the shader, alpha bent `1-(1-a)^2.2` (exact for black sources) |
| Sorting | every camera has `SetOrthographicTransparencySortMode` | `DrawQueue` sorts by depth along the view (+z); shadows 1/64 behind casters; runs placed along the view axis for Bevy's transparent sort |
| Light rig | `Camera/Fix Z (FixWorldZ)/Point Light` in every level | `CameraLight` (light carried by the camera, re-pinned z after shadows read it) |
| Level art | Skyline: 4668 `tk2dSprite`s incl. "Sprite Shadow" copies | `LevelArt`: static sprites, sprite shadows (from the light every frame), **moving sprites** |
| Tiled sprites | `tk2dTiledSprite` (Skyline Sky / Buildings / Bottom) | `draw::tiled_sprite_geometry` = `SetTiledSpriteGeom` + `SetTiledSpriteIndices` + `SetSpriteVertexNormals`; tiled shadows re-dimensioned as `SpriteShadowPositioner.Awake` |
| Parallax | `FakeParallax` (helper perspective camera at z −10), `ScaleSpriteWithCameraDistance` | `draw::Motion` (closed-form projection, TODO(UNCERTAIN) rounding) |
| Polygon surfaces | `PolygonColliderRenderer.SetUp` | `surface::surface_mesh`: Clipper miter ring (1/32, shadows 2/32), edges > 20 split, constrained Delaunay (`spade` for poly2tri), Steiner refinement by `maximumTriangleArea`, inner alpha 1 / ring alpha 0, 1/64 dedup grid. Fixes the dark Lobby (dense vertices for per-vertex light) |
| Text | `tk2dTextMesh`, tk2d fonts | `sticks_sim::text` (word wrap, glyph quads, inline colour commands) |
| HUD | Permanent System `Player Info`, `UI Camera` (ortho, depth 10, layer UI, no clear), `tk2d/BlendVertexColor` (`assets_raw/Assets/Resources/BlendVertexColor.bytes`: unlit `texture × primary`, Cull Off) | `hud.rs`: second Bevy camera (order 10, RenderLayers 1), sprites unlit (tint 0.5 cancels the shared shader's doubling), `HealthUI`, `WeaponIndicatorUI`, `VersusScoreUI` activation, `ViewportAnchor`, `ScaleWithAspectRatioToMaintainWidthFraction`, `BigScreenOrthographicCameraScaler`; `draw::clipped_sprite_geometry` = `SetClippedSpriteGeom` |
| Explosions | `Explosion`, `ExplodeSpritesAndLines`, `Explodable` | `sticks_sim::explosion` + `figure.rs` (see §4) |

## 4. Implemented

| Area | State |
| --- | --- |
| Level data | Platforms/Skyline (and any level scene): colliders, layers, tags, triggers, spawn points, camera, surfaces, lights, `AllowedWeapons`, sprites |
| Physics, animator, character, camera | Static colliders, compound body, grounding/jump/wall-jump/slopes/duck, real Mecanim controller + clips + root motion, `CameraPositioner` |
| Health / lives / respawn | `Health`, `Lives`, `Respawner`, explosion timing, Abyss trigger, campaign respawn at last-but-one safe position, camera placeholder, frags/self-frag, damage credit |
| Character visuals | No skinned mesh: humanoid pose → `LinesToChildren` capsules (+ shadows) and `FlatSprite`s (+ shadows) through the shared queue, lit per vertex; hit flash; head tilt from the avatar's T-pose frame |
| Death explosions | `Explosion.Go`: last frame's lines (with shadow planes) and visible flat sprites fly (`insideUnitCircle × 5`; π rad/s lines, 180°/s sprites via `Rotate(Vector3.back)`), 2 s drift, 1 s fade to transparent black (`Color32.Lerp`/`Color.Lerp`), shadows; `Go` runs `Update` once with the damage callback's delta, then every frame next to the life cycle's clock. Avatar hidden in that frame's `LateUpdate` |
| Weapons / projectiles | Launchers, aim, IK, charge (`SetProjectileSpeedByCharge`), `RotateToFaceVelocity`, enter-once contacts with bisection refinement, `TellOwnerToExplodeOnCollision[UnlessOtherIsDestroyed]`, `DespawnOnExplode` (next frame), `DespawnOnExplosionFinished`, expiry (`DespawnOnSpawnExpiration` / `ExplodeOnSpawnExpiration`), `NetworkPool` reuse + `Prepooler` (explosion prepared only after 3 frames or reuse), Rocket direct-hit (no area damage in the original) |
| Projectile hit FX / sticking | `Explosion.Go` with collision data: parts start at the contact point with the rotation before contact; `sticks` parts pin to what was hit (avatar root rotated ±90° about y, helicopter root, letters, level colliders), follow it, unstick while it is inactive and re-snap when it returns; stuck with nothing → frozen; lines never stick; `fadeWaitTime` per prefab |
| Particles | `Rocket Trail` (world space, 30/s, life 0.93), `Rocket Explosion` (burst 10, circle edge), `Firework Explosion 0..3` (burst 100); `ParticleSystemLines`/`LinesRenderer`/`DotsRenderer` drawing with shadows, `ParticleRotationFollowsVelocity`, `LifetimeParticleSystem`, `DespawnOnParticleSystemDeath`. **No muzzle smoke, casings, sparks or debris exist in the original** |
| Audio | MasterAudio emulated from `Permanent System.unity`: 14 groups, weighted variations, shuffle-without-repeat, retrigger %, limit modes, random volume/pitch, fades, buses, `Mixer.mixer` group gains (dB up the parent chain), 3D log rolloff at the level camera. Call sites: `ProjectileLauncher.firingSound` (characters and helicopter guns), `PlaySoundOnExplode`, `LifetimeSound` (+ fade-out), `PlaySoundOnButtonPress`, Level Select "Paper Slide", `EventSounds` level/lobby music (`PlaylistController` "Music", cross-fade 0.5 s), helicopter engine loops (`heli_loop`, 0.5/1.0 volume, pitch 0.9 + 0.2·min(1,|movement|), "Helicopter" mixer group). Trigger map in `progress.html` |
| Helicopters | H1–H3 (data, flight model, enemy AI, rotors, guns, hit flash, off-screen sleep) as before; **H6 player boarding**: nearest free `ControlSlot` within 10 (2D search, 3D radius), `E` enters (avatar `Disabled`, i.e. `SetActive(false)`), A/D/S/W fly, the left button fires the single-player gun turned to the cursor (shots credited to the player), `E` leaves (not on the entry frame), destruction ejects; avatar back at the "Disembarkment Point" (x/y, own z) with animator and body recreated; camera framed by the "Camera Influencer" (+10, +2.73, `FixWorldOffsetFromTransform` timing); HUD bar shows the helicopter's health; player helicopters' "Target" is targetable by enemies (`IsRootControlledTargetQualifier` sees the pilot) |
| Enemies | As before (Skyline's 21 enemies, AI behaviours, off-screen sleep) |
| Lobby | `Lobby.unity` as an interactive scene: Splash → Network Setup (title) → Play Offline → Control Assignment → Game Summary → Start Game → level (verified by screenshots). Player slots: the keyboard column is the player's slot → `Session.player_index`; spawn point by `PlayerSlotAssignments.OnLoadedLevelEverywhere` (shuffle outside the campaign / `RandomizeSpawnPoints`) and `PlayerSpawner.SpawnAll`; team per mode; default slot colours (`CustomizeColor.DefaultBodyColorForPlayer`, matching helmet, random darker face); HUD "Player N" (team-grouped in Team Versus); `LobbyTeams` lists kept across games. Sounds queued for the audio plugin. Scripts: StartupDirector, SplashScreenAdvance, NoControllers, ActivateExclusively, CameraPositionInfluencer/Positioner, Button (+ mouse picking on BoxColliders), BackButton, ActivateSomethingOnButtonPress, ExitButton, NetworkSetup (offline), ControlAssignment (one controller, heading colours), LobbyEnder (offline: Back only), GameSummary, GameModeSelect, GameModeDisplay, LevelNameDisplay, DifficultyDisplay/CycleDifficulty, LevelSelect (no animations), SmoothDampTowards, MakeGameModeConsistentWithPlayerCount |
| HUD | Original tk2d HUD (see §3) |
| Screens | `AppState::Lobby` default (command-line level path skips it), `Loading`, `InGame`; Escape pause menu (Bevy UI, not original) |
| Debug / dev aids (not original) | F1: collider outlines (overlay camera, RenderLayers 2) + diagnostics line; dev script (`STICK_SCRIPT`: `shot`, `key`, `aim`, `mouse`, `click`, `damage`, `exit`) |

## 5. Verified

`cargo test -p unity_compat -p unity_data -p sticks_sim` (2026-10-26, in the cloud session with only part of the export staged): every test of the new code passes — `sticks_sim` units 148 passed (incl. explosion sticking ×5, weapons contacts ×4, particles ×3, audio ×5, vehicles ×6, lobby ×8), `unity_data` integration `effects` 6, `audio` 4, `helicopters` 1 (boarding data), `shooting` 4, `death_and_respawn` 1. The 21 unit and 15 integration failures there are environmental only (`Man.prefab: rig not recognised` with unstaged script metas; missing scene/prefab files; a stub `spade`) and passed on the full export before (181 passed, 1 ignored). **`sticks_game` was not compiled in that session (crates.io unreachable): run `cargo check -p sticks_game` before trusting it.** Tests read the real export (`assets_exported/ExportedProject`). New in these passes: PCR mesh (ring, split, refinement, alpha), tiled sprite geometry, `FakeParallax`/camera scaling, clipped sprite geometry, explosion timeline, YAML escapes, lobby data, fonts/text.

Runtime (screenshots via the dev script, compared with the original where a reference exists):
- Title screen vs YouTube thumbnail `XxaT0vJLgZ0`: cardboard background, darker panels, clean text, light falloff — matches (the thumbnail's build shows a lock on "Start Network Game" and no Exit button: another edition).
- Skyline vs Steam `ss_32e51c52…`: ruled-paper sunset sky, silhouette buildings with parallax, HUD position/style match; our sky looks a little brighter (different camera spot; open).
- Death explosion sequence, fade and respawn 2.99 s after death; HUD life bar fade and indicator hiding.
- Platforms: surfaces, polygon shadows, lighting coherent (no reference image).

Expected values come from decompiled source and serialized data, not from recordings of the original executable (no golden traces exist).

## 6. TODO(UNCERTAIN) register

1. `fixedDeltaTime` used by the step right after retuning (previous assumed).
2. Run speed from root motion 11.985 vs designed 11.9406 (+0.37 %).
3. Blend-tree duration blending for analog input.
4. Transition duration with non-unit speed.
5. Interrupted cross-fade collapses instead of snapshotting.
6. Missing parameter curves contribute the default.
7. Box2D vs this solver: resting depth, corner rounding, speculative contacts.
8. ~~`Mouse.AbsoluteAim` plane~~ resolved.
9. `SmoothDamp` last-ulp behaviour.
10. ~~Surface lighting; faded border~~ resolved (vertex-lit pass; PCR alpha ring). Remaining: poly2tri vs `spade` triangle order/cocircular ties; Clipper's union for concave offset corners approximated by the miter point.
11. Safe-position tracking ignores `UnsafeRespawnArea`/`HorizontalScroller2D`/`Projectile` colliders.
12. Animator/rigidbody reset on respawn.
13. Whether the respawned avatar's camera influencer runs on the respawn frame.
14. Order of the two `Health.Exhausted` handlers.
15. Default difficulty (0 assumed).
16. Hand IK / head look-at solver (Mecanim's not recoverable).
17. `ZYRoll` axis composition order.
18. Muscle → angle mapping details.
19. Centre of mass from joints.
20. Reference pose for "Based Upon Original".
21. ~~tk2d shader state~~ resolved (vertex-lit LOD 100 pass; HUD `BlendVertexColor`).
22. Arm/leg twist invisible in the stick figure.
23. Hand/foot collider COM offsets.
24. One-frame ordering between `LookAt`, animator IK and the launcher's position read.
25. `Color32` truncation on the hit flash.
26. ~~Enemy camera influencer~~ resolved.
27. Continuous collision for projectiles approximated by substeps.
28. `OnLoadedLevelEverywhere` frame.
29. A reactivated enemy's behaviours start the frame after waking.
30. Animator reset on reactivation.
31. `Random.Range(float, float)` mapping; `Random.insideUnitCircle` algorithm; `UnityEngine.Random` replaced by `GameRng` (clock-seeded in the original anyway); `Explosion.Prepare` draws not reproduced.
32. Targetable order for equal distances.
33. `Awake` order across scene objects.
34. Line of sight ignores Box2D's polygon skin radius.
35. Camera view bounds computed directly.
36. Fixed-function light scale 0.5 (one screenshot) and ambient term (`ambient × vertex colour`).
37. Gamma-space blending emulated in linear space (alpha bend exact only for black sources).
38. Tie-break order for renderers at equal depth (submission order kept).
39. `FakeParallax` written as the projection's closed form.
40. `Screen.dpi` on Windows taken as 96 × scale factor (sets the HUD's size through `BigScreenOrthographicCameraScaler`).
41. HUD `Health.Changed` applied every frame; `WeaponChanged` for the starting weapon.
42. Explosion `Go` delta for damage dealt outside a physics callback.
43. Projectile explosion readiness: a freshly instantiated projectile's explosion is "prepared" only from its 3rd frame (`ExplodeSpritesAndLines.Run` yields one frame after `Start`); reused pool instances are always prepared.
44. Projectile contacts: enter-once with bisection to the contact point; no Box2D deflection of the projectile body after a non-exploding contact.
45. Particles: emission accumulator per frame, `Random` draws for circle/edge shapes, start rotation 0; simulation one frame behind (after `LateUpdate`); rotation-follows-velocity applied before drawing.
46. Audio: no stereo panning of 3D voices; linear music cross-fade; `MasterAudio` RNG is not Unity's.
47. Vehicle `Update` order (avatar search → vehicle exit → `Player.Update` disembark, same frame); reactivated avatar acts from the next frame (else the stale `EnterVehicle` would re-board).
48. Reactivated avatar after disembarking: body at rest and animator restarted (as for respawns), controller fields kept.
49. `Targetable` order: scene helicopters' targets before avatars.
50. Spawn-point shuffle with more active slots than spawn points (exhausted enumerator).

## 7. Known gaps

- Effects: `SpawnLocallyOnExplode` prefabs other than particle effects; seeking rockets (`Seeker` parsed, not steered).
- HUD: Versus score label text, off-screen `Pointers`, control prompts, more than one local player.
- Lobby: LevelSelect card animations, colour customisation and display Men, errors/credits/achievements/high scores/team screens, network screens, gamepad highlight; original Level Loading scene and in-game menu (`menu.rs` is Bevy UI).
- Vehicles: H4 helicopter explosion (and with it the player helicopter's respawn via `RespawnerInstantiator`/`RespawnAtStartPosition`), H5 seated occupants (pilot/passenger figures not drawn), H7 helicopter-vs-world contacts; gunner/passenger seats for extra local players; cars (`CarControl`, `WheelJoint2D`), rockets (`RocketControl`), cannons (`CannonControl`) — all need the Box2D contact/joint solver; the "Enter Vehicle" control prompt.
- Weapons: Shuriken, Pistol, Sword, Spear end to end; `DontExplodeOnCollisionWithMe`.
- Enemies: `Emerge`, `Wave`s/spawners, `EnemyRegistrar` and level goals (`LevelComplete`), cannons, `Seeking Rocket`; moving platforms; `MinimumPlayers` / `MinimumDifficulty` object filtering (not checked in levels yet).
- Audio: stereo panning; level-end applause (waits for the level-end flow).
- Save files (high scores → level unlocks), networking (Unity legacy RakNet: not reproducible offline), versus/team modes end to end, team arrangement screen.

## 8. Next steps

1. `cargo check -p sticks_game` / `cargo clippy` on a machine with crates.io, then run Skyline 2 and board the helicopter at x 546 (`STICK_SCRIPT="... key KeyE down; ... key KeyE up"`).
2. H4 helicopter explosion + respawn, H5 occupants, then H7 (Box2D contacts) which unlocks cars/rockets/cannons.
3. Level flow (`EnemyRegistrar`, level enders, `LevelComplete`, applause), `Emerge`, remaining weapons, persistence.
4. Lobby remainder (card animations, colour customisation, display Men, Level Loading scene, in-game menu).
