# Stick 'Em Up 2 — Bevy reimplementation: implementation map

State on 2026-10-31 (after the network-play, settings-page and standing-pose pass; before that the Japan / Awake-gates / Versus pass; before that the lobby-completion pass; before that the level-end, in-game UI, melee and lobby-music pass; before that the effects, audio, vehicle-boarding and lobby-slot passes, on top of the rendering, lobby, level-art, HUD, explosion, helicopter H1–H3 and gameplay passes). `progress.html` at the project root is the visual audit of these sections. Code lives only in `reconstruction/` (AGENTS.md makes every other folder read-only). Reference build: Steam, Unity 5.1.3f1, Mono. Session-by-session details: `reconstruction/.claude-handoff/<timestamp>/`.

## 0. Rule used to reconcile the two instruction sets

Structure is Bevy-native (components, resources, messages, plugins, states, system sets); observable behaviour is Unity-exact (update order, f32 operator order, fixed-step policy, recovered constants). Where idiomatic Bevy and fidelity disagree, fidelity wins and the choice is recorded here. No fake Unity engine: compatibility code only where it reproduces observable behaviour.

## 1. Layout (cargo workspace in `reconstruction/crates/`)

| Crate | Bevy | Role |
| --- | --- | --- |
| `unity_compat` | no | Unity numerics: `Mathf` (incl. `Lerp`/`Clamp01`), `SmoothDamp`, fixed-step/time policy, `Vec2/Vec3/Quat` |
| `unity_data` | no | Readers for the exported project: YAML (with double-quote escapes), **MasterAudio set-up + `Mixer.mixer` (`audio`)**, **Shuriken particle prefabs (`particles`)**, **helicopters incl. control slot, camera influencer, target, engine `AudioSource` (`vehicles`)**, **built-in meshes, `HorizontalScroller2D`, breakable props (`scenery`)**, **`Awake` gates (`MinimumPlayers`, `MinimumDifficulty`, `ActivateOrDestroyAccordingToDifficultyLevel`: `level::apply_awake_gates`)**, **the Versus Game Ender (`level::versus_ender`)**, scenes → levels (colliders incl. disabled ones that a `PolygonColliderRenderer` still draws, surfaces with `maximumTriangleArea`, lights, camera chain), interactive scenes (object tree, scripts with fields, renderers under inactive objects; local TRS), tk2d sprite collections (texel size, bounds, flip mode, pixels per metre, PNG-bytes atlases) and scene sprites (`tk2dSprite`, `tk2dTiledSprite`, `tk2dClippedSprite`, `FakeParallax`, `ScaleSpriteWithCameraDistance`, shadows), tk2d fonts/texts, clips, animator controller, Physics2D settings, materials, humanoid avatar, `Man` rig + weapons, projectile prefabs |
| `sticks_sim` | no | Rules and geometry: collision world, character controller, animator, pose solver, camera rig, life/death/respawn, stick-figure geometry (`visual`), aim + IK, weapons, enemy AI, **draw queue + vertex lighting (`draw`)**, **`PolygonColliderRenderer` mesh (`surface`)**, **tk2d text (`text`)**, **explosions incl. sticking (`explosion`)**, **particles (`particles`)**, **MasterAudio voices/fades/rolloff (`audio`)**, **`ControlSlot` boarding rules (`vehicles`)**, **lobby teams / spawn-slot / colours and swatches (`lobby`)**, **posed lobby models (`lobby_models`)**, **achievements (`goals`)**, **`DataManager` save files (`save_data`)**, helicopters (`helicopter`), projectiles (`weapons`), **level enders, scores, high scores, tips, UI fades (`level_end`)**, **melee swings (`melee`)**, **input processors and `RoleInput` choice (`devices`)**, **`ControlAssignment` (`control_assignment`)**, **versus end rules and score table (`versus`)**, **scrollers and prop parts (`scenery`)**, **`Emerge` (`ai::Emergence`)** |
| `sticks_net` | no | Network play without the engine: `ServerDiscovery` announcements (the original's packet format), framed TCP host/client plus `Link`s (connections another transport carries, through channels), `GameAddress` (LAN end point or online game code), the message codec (one message per RPC / state stream), `PlayerSlotAssignments` (`slots`) |
| `sticks_p2p` | no | Internet play, not in the original: an iroh endpoint (QUIC, hole punching, n0 relays as fallback, address lookup by public key) on a 2-thread tokio runtime; the host accepts game connections and answers probes; connections become `sticks_net::Link`s (length-prefixed frames on one stream, empty frame each second as keep-alive) |
| `sticks_launcher` | **0.19** | The launcher (not in the original): a cardboard screen drawn from the Lobby's own art (root Background surface, Title sprite, the title-screen buttons' paper, their tk2d font and shadows) as unlit 2D meshes; cheats, settings, player name; starts `sticks_game` next to it with `STICK_LAUNCH_OPTIONS` |
| `sticks_game` | **0.19** | App: states, plugins, devices, scheduling, rendering. Modules: `level`, `player`, `camera`, `life`, `figure`, `weapon`, `enemy`, `helicopter`, `vehicles` (`VehiclePlugin`), `level_end` (`LevelEndPlugin`), `ingame_ui` (`InGameUiPlugin`), `effects` (`EffectsPlugin`), `audio` (`GameAudioPlugin`), `hud`, `lobby` (`LobbyPlugin`; `lobby/figures.rs`, `lobby/screens.rs`), `save` (`SavePlugin`), `input` (`InputPlugin`: gamepads, mouse activity), `versus` (`VersusPlugin`), `scenery` (`SceneryPlugin`), **`net` (`NetPlugin`), `lobby/network.rs`, `settings` (`SettingsPlugin`), `lobby/settings_page.rs`**, `unity_render`, `menu`, `debug`, `devscript`, `state`, `config` |

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
| Sorting | every camera has `SetOrthographicTransparencySortMode`; `TagManager` sorting layers (In-Game, Background, Button Background, Default, Front …) on the UI renderers | `DrawQueue` sorts by sorting layer rank and order, then depth along the view (+z); shadows 1/64 behind casters; runs placed along the view axis for Bevy's transparent sort (level renderers all rank 0) |
| Light rig | `Camera/Fix Z (FixWorldZ)/Point Light` in every level | `CameraLight` (light carried by the camera, re-pinned z after shadows read it) |
| Level art | Skyline: 4668 `tk2dSprite`s incl. "Sprite Shadow" copies | `LevelArt`: static sprites, sprite shadows (from the light every frame), **moving sprites** |
| Tiled sprites | `tk2dTiledSprite` (Skyline Sky / Buildings / Bottom) | `draw::tiled_sprite_geometry` = `SetTiledSpriteGeom` + `SetTiledSpriteIndices` + `SetSpriteVertexNormals`; tiled shadows re-dimensioned as `SpriteShadowPositioner.Awake` |
| Parallax | `FakeParallax` (helper perspective camera at z −10), `ScaleSpriteWithCameraDistance` | `draw::Motion` (closed-form projection, TODO(UNCERTAIN) rounding) |
| Polygon surfaces | `PolygonColliderRenderer.SetUp` | `surface::surface_mesh`: Clipper miter ring (1/32, shadows 2/32), edges > 20 split, constrained Delaunay (`spade` for poly2tri), Steiner refinement by `maximumTriangleArea`, inner alpha 1 / ring alpha 0, 1/64 dedup grid. Fixes the dark Lobby (dense vertices for per-vertex light) |
| Text | `tk2dTextMesh`, tk2d fonts | `sticks_sim::text` (word wrap, glyph quads, inline colour commands) |
| HUD | Permanent System `Player Info`, `UI Camera` (ortho, depth 10, layer UI, no clear), `tk2d/BlendVertexColor` (`assets_raw/Assets/Resources/BlendVertexColor.bytes`: unlit `texture × primary`, Cull Off) | `hud.rs`: second Bevy camera (order 10, RenderLayers 1), sprites unlit (tint 0.5 cancels the shared shader's doubling), `HealthUI`, `WeaponIndicatorUI`, `VersusScoreUI` activation, `ViewportAnchor`, `ScaleWithAspectRatioToMaintainWidthFraction`, `BigScreenOrthographicCameraScaler`; `draw::clipped_sprite_geometry` = `SetClippedSpriteGeom` |
| In-game screens | Permanent System `In-Game UI`: "Paused (Offline)", "Tips", "End of Level", "Game Over" (`tk2dSlicedSprite` panels, `tk2dTextMesh` captions, UI atlas and "Regular data" font share the `BlendVertexColor` material) | drawn by `hud.rs` with the same camera: `draw::sliced_sprite_geometry` = `SetSlicedSpriteGeom` + `SetSlicedSpriteIndices`, texts via `sticks_sim::text`; behaviour in `ingame_ui.rs` (see §4) |
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
| Helicopters | H1–H3 (data, flight model, enemy AI, rotors, guns, hit flash, off-screen sleep) as before; **H6 player boarding** and **H7 contacts** (Box2D 2.3 port: polygon/circle manifolds, sequential impulses with warm starting and friction, Baumgarte position correction, mass and inertia from the hull scaled to `Rigidbody2D.mass`; helicopters collide with the level and each other on the layers the matrix pairs, and characters collide with hulls on their layers: enemy soldiers with any helicopter, players only with enemy ones): nearest free `ControlSlot` within 10 (2D search, 3D radius), `E` enters (avatar `Disabled`, i.e. `SetActive(false)`), A/D/S/W fly, the left button fires the single-player gun turned to the cursor (shots credited to the player), `E` leaves (not on the entry frame), destruction ejects; avatar back at the "Disembarkment Point" (x/y, own z) with animator and body recreated; camera framed by the "Camera Influencer" (+10, +2.73, `FixWorldOffsetFromTransform` timing); HUD bar shows the helicopter's health; player helicopters' "Target" is targetable by enemies (`IsRootControlledTargetQualifier` sees the pilot) |
| Enemies | As before (Skyline's 21 enemies, AI behaviours, off-screen sleep) |
| Lobby | `Lobby.unity` as an interactive scene: Splash → Network Setup (title) → Play Offline → Control Assignment → Game Summary → Start Game → level (verified by screenshots). Player slots: every control-assignment column with an available input is a player → `Session.players` (index + its inputs); spawn point by `PlayerSlotAssignments.OnLoadedLevelEverywhere` (shuffle outside the campaign / `RandomizeSpawnPoints`) and `PlayerSpawner.SpawnAll`; team per mode; default slot colours (`CustomizeColor.DefaultBodyColorForPlayer`, matching helmet, random darker face); HUD "Player N" (team-grouped in Team Versus); `LobbyTeams` lists kept across games. Sounds queued for the audio plugin. Scripts: StartupDirector, SplashScreenAdvance, NoControllers, ActivateExclusively, CameraPositionInfluencer/Positioner, Button (+ mouse picking on BoxColliders), BackButton, ActivateSomethingOnButtonPress, ExitButton, NetworkSetup (offline), ControlAssignment (`sticks_sim::control_assignment`: `Start` puts the keyboard and a lone gamepad in Player 1, new inputs take the first empty column, left/right per input, right of Player 4 = unused, left from there = Player 4, one card per input instantiated from `prototypeInputButton`, stacked at `ButtonY(row)` = start − 1.2 × box height × row, drag and drop between headings, instruction labels, heading colours), TeamArrangement (a card per active player under its team heading, drag or the player's own left/right moves it), LobbyEnder (offline: Back only), GameSummary, GameModeSelect, GameModeDisplay, LevelNameDisplay, DifficultyDisplay/CycleDifficulty, LevelSelect (card pile animation, sliding name/number/top-score labels, difficulty stars and high-score button from the saved scores), SmoothDampTowards, MakeGameModeConsistentWithPlayerCount. **Lobby completion (2026-10-27, `lobby/screens.rs`, `lobby/figures.rs`)**: `Object.Instantiate` of scene subtrees (`Lobby::instantiate`, re-parenting rebased); `CustomizeColor` ×3 (Body/Helmet/Face: Default/Match/Random button + 15/15/20 swatch copies, player label under the chosen swatch, A/D or a click changes it, saved per `PlayerID`) and the chosen colours used by `PlayerSpawner`; `ColorModels` with the posed `Man` models (real `Character` physics on the Lobby's colliders, idle animator, Shuriken from `AllowedWeapons` aimed along (1.91, 0.51), colour fade 15·Δt through `Color32`, summary gaps closed / grouped by team with the "vs" label); `FollowX`/`FollowActivation`; `LobbyError` (Shakespeare message, Okay returns); `HighScoresDisplay` (10 rows, golden colour, backs out outside the campaign); `GoalsDisplay` ×3 (rows sized by the wrapped description, dash / dot / pie marks per player via `LinesRenderer`) and `OverallGoalProgressDisplay` ("n/12 Done"); goals recorded at level end (`complete d`, `golden d`); `ShowAchievements`; credits `WebLink` (hover ×1.2 / press ×0.9, opens the URL), `VersionNumberLabel`, `AllLevelUnlockCheat`, `TrivialFPSEnabler` (+ FPS label), `ShowEndingCheat` (ending scene not reconstructed); `DataManager` files `Goals.dat`/`Scores.dat`/`Colors.dat`/`Tips.dat` in the original byte format (`STICK_SAVE_DIR` or the user data folder). **Gamepads** (`input.rs` + `sticks_sim::devices`): every Bevy gamepad takes the lowest free index ("Xbox Controller N"), `XboxishGamepadLobbyInput` (stick dominant axis or d-pad, edge-triggered; A select, B/Back back), `Button` highlighting (`isDefault`, `up/down/left/right` passing over switched-off neighbours, `returnToLastSelected`, `OnDisable` hand-on; highlighted text (255, 143, 56)); `CustomizeColor` per player's own inputs, mouse clicks for the keyboard-and-mouse player. Not reproduced: touch input |
| HUD | Original tk2d HUD (see §3), one `Player N` per player (Team Versus: grouped by team), `VersusScoreUI` frags (definitive once the game ends), Team Versus `Team Info` (`TeamVersusScoreUI`: team colour = mean body colour, centred over its players, team frags) |
| Screens | `AppState::Lobby` default (command-line level path skips it), `Loading`, `InGame`. **In-game screens are the original's** (`ingame_ui.rs`): `InGameMenuHandler` (Escape opens "Paused (Offline)" unless tips / game over / end of level show, Escape or "Resume Game" fades it out, "Show Tips" fades it out then shows the tips, `CloseAllMenusImmediately`), `Pause` (timeScale 0; the tips' via `EnableBehaviourWhenOffline`), `FadeInUI`/`FadeOutUI` (alpha of every sliced sprite and text below, SmoothDamp 0.25 s in / 0.1 s out, unscaled), `Button` mouse events on the `CopyBoxColliderFromChild` box (down / enter / up-as-button, pressed z tween, `ScaleProportionallyToLocalPositionZ`), `BackButton` (Escape on the tips' "Okay"), `FadeOutUIOnButtonPress`, `RestartLevelButton` (reload), `ReturnToLobbyButton`, `LevelCompleteButton` (next campaign index → Lobby; last level → index 0, ending scene not reconstructed → Lobby), `LevelTips.StartLevel`/`ShowTips` (first tip whose control types are all in use; shown once per player with "View Again Later", for the session). `Time.timeScale` is `menu::TimeScale` → `Time<Virtual>` |
| Level end | Every campaign level's "Level Ender" (`unity_data::level::level_enders`) | `level_end.rs`: `ActivateSomethingOnTriggered` + `TriggeredByAllPlayers` (marker trigger circle; enter/exit from the character's trigger overlaps, game overs), `DestroyAllEnemiesLevelEnder` (`EnemyRegistrar` count), `DestroyParticularEnemyLevelEnder`; the Ender's `EndOfLevelUIActivator` (1 s scaled, then `LevelTimer.LevelTime`), `EndOfLevelCameraMover` (margin 0, width 0, distance 14), `EndOfLevelTimeSlower` (→ 0.4, unscaled, keeps running), `DisableGameOvering`, `LevelComplete` (`LevelTimer`, `EndOfLevelEnemyDestroyer`), `ExplodeSomethingOnStart` (marker lines `Explosion.Go`), `EventSounds` "Applause"; `GameOverUIActivator` → "Game Over" (Restart / Quit and Return to Lobby); the marker's `PolygonColliderOutlineLines` (white 0.28 lines + shadows); `MaximumDamageCalculator`; `ScoreCalculator` (rows by player count, box resized, tick-on 400/33/3 per frame, golden cells) and `HighScores` tables (`ScoreTable` top 10 with `ScoreSorter`, personal bests) → `HighScores.IsLevelUnlocked` in the Lobby's level select (session only) |
| Melee | "Sword Swish" (`DamageOtherOnCollision` 40, `SpawnExpiration` 0.25, `FollowProjectileLauncher`, kinematic; "Deflector" with `DontExplodeOnCollisionWithMe` and "Sword Deflector" bounciness 1; "Damager"; `TakeResponsibilityForCollidingDamagers`) | `unity_data::projectile::try_load_melee`, `sticks_sim::melee::Swing`: spawned by the launcher at the launch point turned to the fire direction, moved to it every physics step, damages each body on a colliding layer once per new contact (≤ once per step), bullets touching the deflector bounce off (restitution against the moving body, Box2D 1 m/s threshold) and become the swinger's unstoppable shots |
| Local multiplayer input | `PlayerSpawner.Spawn` gives each player's roles the processors of its column (`PlayerDevices`); `RoleInput.Update` (`sticks_sim::devices::RoleInputChoice`): processors in order until one is ACTIVE, else the current one, else the idle one ranked by `ProcessorPrioritizer` (gamepad before keyboard); none → cleared input, `DefaultAim`. `KeyboardAndMouseCharacterInput` (active on movement > 0.05, buttons, wheel, E, mouse moved last frame) and `XboxishGamepadCharacterInput` (left x past 0.2, duck at stick-down 0.4, LT ≥ 1/256 jumps, RT fire trigger with rise/fall hysteresis, right stick past 0.2 sets the kept relative aim, Y next weapon, A enters vehicles). Helicopters: `KeyboardAndMouseHelicopterInput` (gun at the cursor) and `XboxishGamepadHelicopterInput` (left stick past 0.2 flies, RT fires, right stick past 0.2 aims relative to the helicopter, A leaves), chosen per pilot like the character processors; `IsPilotAlsoAGunner` = one active player. In-game menus: `XboxishGamepadMenuInput` (Start opens, B/Back/Start dismiss). Level texts by `AllClientControlTypes` (`ControlTypeDependentActivator`), tips by every player's control types |
| Network games | `crate::net` + `sticks_net` + `lobby/network.rs`. Connection screen: "Start Network Game" hosts on 25790 (announced on UDP 25791 every second as `SEU2\0<version>\0<port>\0<machine name>`; this game announces version 1005 so it and the original see each other's hosts but refuse to join across, with the original's version messages); "Join Game: NAME" / "Show Network Games (N found)" / the paged Server List; `LobbyEnder` Back/Disconnect/Stop Server; "connection … lost" error. `PlayerSlotAssignments` claims (host grants, "assigned elsewhere" labels, columns played elsewhere skipped by left/right and drops), `ResolveWithServerAssignments`; mode/level/difficulty/teams and colours shared. `LevelLoader`: the host loads for everybody (`LoadLevel` with the scene path, mode, difficulty, player count, prefix and a shared seed), gameplay waits for `AllFinishedLoadingLevel`, clients' Restart / Return to Lobby / Next Level go to the host, `maxConnections` 0 in a level. In a level each machine owns its players' avatars (and the helicopter one of them flies); the host owns enemies and other helicopters. Owners stream every frame: `CharacterInput` (movement, jump, duck, fire, aim, enter-vehicle edges), position (pulled a tenth of the way per fixed step, snapped beyond 10 units: `networkTargetPosition`), layer, z, equipped weapon (`SetWeapon`), activity (enemies asleep off screen); helicopters: input + hull. Host-only: damage (`DamageOtherOnCollision` on the server; clients take `SyncServerDamageToClients` health), scores (`ReceivePlayerScore`), enemy AI and off-screen sleep, level enders (`LevelEnder` event), versus end (with the definitive frags). Server/client pause menus, `InGameDisconnectButton`, tips don't pause online, a client that loses its host returns to the Lobby, a host removes a departed client's players (`RemoveDisconnectedPlayers`). Internet play, peer to peer (not in the original): a host also takes clients by its iroh game code; "Send Link" (hidden by the original on desktop) copies `seu2://<code>/<name>`; a machine whose clipboard holds an invitation probes that host every 4 s while a listing screen is up and lists the game only while the host answers (its own code never), so stale links are not listed; joining it connects over QUIC (direct or relayed). A second copy of the game on one machine hosts for the internet only when the LAN port is taken |
| Settings page | Not in the original: a "Settings" button (a copy of the title screen's Exit button mirrored to the top left, its paper and click box widened 1.9×) opens a page built from copies of the Server List screen's pieces: music volume, sound volume (100/75/50/25/off), fullscreen, vertical sync, Show FPS (`TrivialFPS`). Saved as `Settings.txt` beside the save files (`sticks_sim::options`, shared with the launcher) |
| Launcher | `sticks_launcher` (not in the original): cheats (god mode, infinite lives, rapid fire, all weapons, unbreakable helicopters, game speed, cheat-key list: need the game's local `hacks.rs`, re-applied at the start of every level; Unlock All Levels = the original's `AllLevelUnlockCheat`), settings (`Settings.txt`), player name (announced to other machines instead of the machine name). Choices kept in `Launcher.txt`; passed to the game in `STICK_LAUNCH_OPTIONS` |
| Versus / Team Versus | `VersusGameEnder` (`versus.rs`): 10 frags (a player in Versus, a team's total in Team Versus) → definitive frags frozen, no more respawns (`GameEnded`), the Ender: `EndOfVersusGameUIActivator` (1 s scaled → `CloseAllMenusImmediately` → "End of Versus Game" / "End of Team Versus Game"), `EndOfLevelCameraMover`, `EndOfLevelTimeSlower`, `EventSounds`, `EndOfVersusGameLoserDestroyer` (losing teams' avatars or flown helicopters take `float.MaxValue`). `VersusScoreTable` (`sticks_sim::versus::ScoreTableLayout`: frags-against matrix with 3+ players, hidden rows/columns, Total column, table centring, bottom/contents shift, border − 543.5 px/unit, min width 917.9, "{name} Wins!") and `TeamVersusScoreTable`; "Continue" returns to the lobby |
| Japan / Awake gates | `MinimumPlayers` (helicopter guns, Moon Race spawn points and rovers), `MinimumDifficulty` (Japan enemies), `ActivateOrDestroyAccordingToDifficultyLevel` applied before extraction (`load_level_for` with `AwakeConditions`); sky plane (built-in Plane mesh, "Color Polygon" material) and mountains with `FakeParallax`; 27 drifting `HorizontalScroller2D` clouds (wrap then move each fixed step; sprites follow); 27 breakable hanging lamps (`Health` 5, `SetLayerInCampaignMode` 30, `LevelColliderHit` damage → `ExplodeSpritesAndLines` of their `SpriteShadower` sprites + "Small Explosion", colliders off); `Emerge` enemies (hidden at z 0 on layer 25 until their `TriggeredByAnyPlayer` trigger, walk to `emergePointX`, then z −0.1, own layer, 0.4 s wait, fire-and-move released) |
| Debug / dev aids (not original) | F1: collider outlines (overlay camera, RenderLayers 2) + diagnostics line; dev script (`STICK_SCRIPT`: `shot`, `key`, `aim`, `mouse`, `click`, `damage`, `exit`); `build.rs` compiles in a local, git-ignored `src/hacks.rs` when present (dev hooks it uses: `life::Invulnerable`, `Armament::unlock_all_weapons`/`launchers_mut`, `Launcher::set_firing_rate_time`, `Lives::set_remaining`, `Helicopter::refill_health`) |

## 5. Verified

`cargo test -p unity_compat -p unity_data -p sticks_sim` (2026-10-26, in the cloud session with only part of the export staged): every test of the new code passes — `sticks_sim` units 148 passed (incl. explosion sticking ×5, weapons contacts ×4, particles ×3, audio ×5, vehicles ×6, lobby ×8), `unity_data` integration `effects` 6, `audio` 4, `helicopters` 1 (boarding data), `shooting` 4, `death_and_respawn` 1. The 21 unit and 15 integration failures there are environmental only (`Man.prefab: rig not recognised` with unstaged script metas; missing scene/prefab files; a stub `spade`) and passed on the full export before (181 passed, 1 ignored). 2026-10-26 (later): `contact` 7 unit tests + `physics` moving-obstacle test + `tests/helicopter_collisions.rs` (the Skyline 2 player helicopter flown down for 8 s settles on the roof it is parked on) pass. **`sticks_game` was not compiled in that session (crates.io unreachable): run `cargo check -p sticks_game` before trusting it.** 2026-10-27: `level_end` 10 + `melee` 2 + sliced-sprite 1 + sorting-layer 1 unit tests, `unity_data` `level_end` 3 (Skyline marker/ender, Office enemy count, Pirates tip) and `weapons` 5 (incl. the Sword Swish melee data) pass; the 2 remaining `sticks_sim` failures are the stub `spade` (surface tests). `sticks_game` again not compiled here. 2026-10-27 (lobby pass): `lobby` 9 (swatches, closest colour, custom spawn colours), `lobby_models` 2 + `tests/lobby_models.rs` (summary and customisation models land on their ground in the real `Lobby.unity`, posed with the Shuriken), `goals` 2, `save_data` 3 (byte-exact .NET strings, scores/colours/goals/tips round trips) pass; 4 `sticks_sim` failures are environmental (stub `spade`; level sprite collections not staged). `sticks_game` not compiled (crates.io unreachable) — `cargo check -p sticks_game` first. 2026-10-27 (Japan / Versus pass): `devices` 6, `control_assignment` 4, `versus` 4, `scenery` 2, `ai` emergence 1, `draw` built-in plane 1 unit tests and `unity_data` `helicopters` 3 (gun gates by player count, Japan difficulty gates), `japan` 1 (sky, mountains parallax, 27 scrollers, 27 lamps, ≥ 15 emerging enemies with triggers), `versus` 1 (Japan VS / Skyline VS / Treetops VS enders) pass; failures are the known environmental ones (stub `spade`, unstaged sprite collections and `NetworkSetup` meta). `sticks_game` not compiled (crates.io unreachable) — `cargo check -p sticks_game` first. Tests read the real export (`assets_exported/ExportedProject`). New in these passes: PCR mesh (ring, split, refinement, alpha), tiled sprite geometry, `FakeParallax`/camera scaling, clipped sprite geometry, explosion timeline, YAML escapes, lobby data, fonts/text.

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
16. Hand IK / head look-at solver (Mecanim's not recoverable); the arm may stretch by the avatar's `m_ArmStretch` (5 %) to reach a grip (stretch curve assumed linear). The torso does not lean (checked against the original by the user).
17. `ZYRoll` axis composition (`normalize(tx, ty+tx·tz, tz−tx·ty, 1)` of the half-angle tangents, recalled, not decompiled).
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
51. Rigid-body contacts (`sticks_sim::contact`): edges as two-sided two-vertex polygons, two-point manifolds solved point by point (no block solver), default material for every collider (friction 0.4, bounciness 0), no time of impact, no sleeping.
52. Characters meet helicopter hulls as moving obstacles placed at the start of the frame and push nothing back (Box2D would exchange impulses between the two rigid bodies).
53. `ScoreCalculator`'s coroutine order inside a frame: the totals are summed before that frame's column ticks (they trail by one frame); `Application.targetFrameRate` -1 (60 expected).
54. An avatar deactivated inside a `TriggeredByAllPlayers` trigger sends no `OnTriggerExit2D` (stays counted).
55. Level-enders' activators all share one `toActivate` (true in every shipped level).
56. The sword's "Damager" is not tested against projectiles (in the original a bullet touching it, not the deflector, would explode); deflection is a one-shot reflection off the convex deflector.
57. `MaximumDamageCalculator`: the soldiers' and enemy helicopters' starting health; `DamagingDoesNotAwardScore` objects (none recovered yet) are not excluded.
58. The first `OnEnter(Lobby)` runs before `Startup`: the Lobby music cue is started from `Update` once MasterAudio is loaded (that ordering was why the menu music never played).
59. Lobby models: `DeactivateOffScreen` is disabled on them and their targeter finds nothing, so they are simulated as idle characters; models of every screen run from the start (as the original's active objects). Characters do not collide with each other.
60. `TrivialFPS`' `GUIStyle` is not recovered (Bevy's default text); `interval` taken as the script default 0.5.
61. Offline every slot's `PlayerID` is the anonymous local one ("Player N"); slot 0 would be the Steam user's with Steam running.
62. `ScoreTable.GetScores` re-sorts on every read in the original; scores are kept sorted on insert here (same result). Save files are written in sorted key order rather than .NET dictionary order (contents identical).
63. Built-in Plane UVs/orientation for the Japan sky (only its colour is drawn: "Color Polygon" has no texture).
64. Props whose `SpriteShadower` sprites span several atlases explode with the first atlas's texture.
65. `Emerge`'s 0.4 s wait counted in frame deltas (one frame either way on float accumulation).
66. Gamepads: every pad is an XInput "Xbox Controller N" (the original also lists "preconfigured" joysticks as "Controller N"); indices are the lowest free one (XInput user index stand-in); Bevy's own stick deadzones apply before the original's 0.2.
67. `LobbyInput.All` order (keyboard first, then gamepads by index) and a player's processor order (same).
68. `XboxishGamepad.Update` edges computed every frame (the original only on frames a processor runs).
69. `ControlAssignment.Start` runs the first time the screen is up (after `LobbyInputManager.Ready`); a gamepad that reconnects with a new index is a new input.
70. `VersusGameEnder` runs in `LateUpdate` (`FrameSet::Present`); the Ender's `Start`s run the same frame (Unity: next frame). Losers flying a helicopter: the helicopter takes the damage (`Player.Controlling`).
71. `EndOfVersusGameUIActivator`'s `WaitForSeconds(1)` uses scaled time while `EndOfLevelTimeSlower` slows it (as in the campaign).
72. Idle's root orientation: `m_OrientationOffsetY` (20°, Idle only) is taken to turn the clip's 29° heading back to 9°; the original's standing figure is near side-on with both legs from one point (49° splays them).
73. A grounded remote character: the original subtracts last step's network pull from a velocity the root motion had already replaced, stalling the pull at 1/11; the port leaves it out so the pull converges.
74. Network: `Application.internetReachability` taken as "on a LAN"; a client's own Disconnect is treated as deliberate (no error screen); a failed connection shows the "connection … lost" error; a level's random draws share the host's seed.
75. Internet play is an addition: an iroh QUIC stream replaces RakNet's reliable channel (head-of-line blocking on loss; the state streams could move to QUIC datagrams); game codes are random per run (the host's public key).

## 7. Known gaps

Done in the 2026-11-01 pass (built on Windows by the user; see the session log for checks):
`Seeker` steering (Corruption 2's seeking rockets: nearest player or player-helicopter
"Target" at spawn, turned by `maximumAngularAcceleration` each fixed step); `TimeLimit` with
the level's "Timing UI" (Moon Race) and `DestroyAllEnemiesLevelEnder.countdownLabel` (Office):
the level's UI-layer objects are grafted into the HUD scene (`InteractiveScene::graft`,
`LayoutControls`, `TimerUI`); `TriggeredByAllPlayers` counts a helicopter's pilot (hull
overlap, `Body::overlapping_triggers` with polygon pieces); H4 helicopter explosion
(`ExplodeSpritesAndLines`, `PlaySoundOnExplode`, ejection) with the player helicopter's respawn
at its start and enemy helicopters destroyed; projectile sprites keep their out-of-plane tilt
(the Rocket's halves at ±15° about x, shaded differently); weapons a level lacks borrow sprites
from another collection (All Weapons cheat, not in Original Game Only); Treetops' collection
with an empty sprite slot loads (it failed before and hung the load). Additions: room codes
(Send Link hidden as in the original), the launcher's Play Original button from Steam's
`%command%`, the original's icon in both programs (`exe_icon`).

Moon fixes (same day, after the first car build): `GravityChanger` (Moon and Moon Race run at
(0, −8.055) instead of the project's −16.076; characters' jumps and the rovers follow it); a
`PolygonColliderRenderer` on another object that names a collider draws it (Moon Race's track
`Ground` is drawn by its "Base Visual" child; before, only the strip of ground sprites showed);
`TriggeredByAllPlayers` and `TimeLimit` count the player driving a car (any chassis or wheel
collider; Moon Race could not be finished before); `VehicleOccupant` figures (the driver of a
car and the pilot of a helicopter in the player's colours while driven; enemy pilots and
passengers the scene enables), posed in "Driving"/"Sitting" with `Animator::play`; the
weapon-test target appears only on the development level `Platforms` (it stood in Moon Race).
Checked: the rover faces right as in the scene (seat at the back, dish over the front wheel);
W/S set its speed and A/D tilt it, as `KeyboardAndMouseCarInput` does. Room UI: a square server
button (bottom left) opens an online page (create, join by code, members, chat); rooms no longer
wait for "ready".

Level-logic pass (2026-11-01, later): Pirates' `Whale`s rise 14 units and sink on their timers
(`SmoothStep` over `transitionTime`), as moving obstacles the characters stand on, with their
sprites; Moon's `StarShower` drops "Star" bodies (damage 20, ×2 by difficulty in the campaign)
on a random target — the players, the rovers' and helicopters' "Target"s — at the x-dependent
rate (×10 rate and spread on a rover, × number of targets), from where they fall onto the target
under the Moon's gravity (host only; clients see them through the usual projectile simulation
only when the host's spawns reach them — TODO(UNCERTAIN)); Corruption 2's "Big Bad" explodes
constantly (`ConstantExplosion`: its lines and sprites fly apart every 1/6 s, at most
`ceil(3 / interval)` at once). TODO(UNCERTAIN): the boss's `ColorCycler (period 10)` is not in
the decompiled source, so its colours stay put; projectiles pass through whales (moving
obstacles only stop characters).

Weapons (same day): the firing animations of the weapon pivots' own `Animator`s
(`firingAnimator.SetTrigger` on every shot) play from the shot: "Sword Swing" thrusts the right
grip 0.53 forward and back and turns the blade, "Shuriken Throw" and "Spear Throw" move the grips
(`unity_data::anim::TransformClip`, applied below the weapon pivot after the aim and before the
hand IK; the launch point, and so the sword's swish, rides on the grip). TODO(UNCERTAIN): the
pivots' controllers were not exported, so each clip is bound to its weapon by name and played
once per shot. A projectile meeting the swish's body (not its `DontExplodeOnCollisionWithMe`
deflector) explodes there, so struck bullets drift in mid-air as in the original screenshots.
Moon's lunar module: a wheel-less `Car` driven by `RocketControl` (W throttle `Clamp01`, mass ×
20 along its up axis from the "Rocket" engine; A/D move the spin towards ∓60°/s at 135°/s²), its
`Health` inert (nothing explodes it), three `PassengerControl` seats taken once the pilot's is
(each leaves on its own E); pilot and passengers all count for `TriggeredByAllPlayers`, so flying
everyone up to the command module's marker ends the level. Not yet: its exhaust particles and
engine sound, `GroundRaycastCameraInfluencer`, and `Dock` (the docking pull after the end).
Level pass, Pirates first (2026-11-01, later): the deck cannon (`crate::cannon`: `CannonControl`
on a non-exclusive slot with `IdleControllingCharacter`: E takes it while the character stays,
crouched with idle input; the mouse aims the "Pivot", the left button fires its 0.7 s
fully-automatic cannonball launcher with the operator's team and credit, `CannonColor` tints the
barrel with the operator's body colour; E lets go); whales are never a safe respawn place
(TODO(UNCERTAIN): the original records them like any ground, which respawned players over the
sunk whale into the sea again and again; the likely cause of the camera "floating away");
the "Enter Vehicle" control prompt ("Use [E]" / the gamepad's, `ControlPrompt` under
`Input/Non-Touch Control Prompts`, laid out by `LayoutControls`) shows while a vehicle or the
cannon can be entered; the Spear's `AimVisualizer` draws the charged throw's dashed arc (kept up
to `MaximumDifficultyLevelComponents`' difficulty 0, TODO(UNCERTAIN): that script is not in the
decompiled source); a car's driver (or passenger) has the car's health on the HUD bar; the game
writes `SEU2_log.txt` next to itself even when started without the launcher.
Skyline / Skyline 2 gunner seats: each player-helicopter gun with its own `ControlSlot`
(`HelicopterGunnerControl`) is a seat further players board with E / A once the pilot's seat is
taken (`CanControl`); the gunner aims and fires that gun (their own team and score credit), their
`VehicleOccupant` shows them in their colours, E / A leaves (`controllerJustSet` latch), the
explosion ejects everyone, and `MoveToOtherControlSlotIfAvailable` moves a gunner into the pilot's
seat when it is free. Vehicle triggers (level enders, `TimeLimit`) count pilot, gunners, driver
and passengers. TODO(UNCERTAIN): gunners use the helicopter role's processor selection (the
`HelicopterGunnerInput` processors read the same buttons) and get out at the helicopter's
disembarkment point.
Office `DestroyAllForGoal`: in a one-player game, breaking all 15 equipment props gives the
"office all destroyed" achievement ("Break room"). The "Complete" ending scene now loads after the
last campaign level (offline, in a network game through the host, and from the credits'
`ShowEndingCheat`) as an ordinary level: its cannons, texts and spawn points; `HidePlayerInfo`
switches the HUD's `Player Info` off; `NextGoalPrompt` picks its text (all achievements done /
Expert at difficulty ≥ 1 / the achievements hint) and fades in after 5 s over 5 s.
Moon vehicles: the rover's gunner seat (`CarGunnerControl`): a second player boards the
"Gunner" once the rover has a driver, sits holding the Rocket Launcher (`SetUpNewWeapon` on the
occupant, arms on the grips), aims it all the way round (`CorrectAndSetAim(faceRight,
canAimBackwards)`) and fires it with their own team and credit; E / A or the explosion gets them
out; vehicle triggers count them. The rover's engine loop (`CarControl`: volume 1 while driven,
pitch 0.48 + 0.17 |speed|) and the lunar module's (volume = throttle) play through the vehicle
engine voices; the lunar module's "Emissions" particles (`engineParticles.emissionRate =
throttle × engineParticleRate`) leave the engine turned with it. The rover's flip goals
("rover 720", "rover 1440") are awarded on landing. Particle cones now spread by their angle
(they all left straight along +z before; TODO(UNCERTAIN) Unity's exact distribution).
`Pointer` (off-screen pointers) is dead code in the original: `SetScreenPosition` has no caller.
From the scripts the user supplied (missing from the decompiled source):
* Camera parent: `CameraPositioner` writes `localPosition`, and in Corruption 2, Moon, Moon Race,
  Pirates and Treetops the camera sits under "Camera Offset" at y 6 / 6 / 6 / 5 / 3: the framed
  centre is now raised by it (the rig works in local space, `CameraRig::parent_offset`). Before,
  these levels were framed that much too low.
* `CameraOffsetXChanger` (Pirates, x 253 → 13, x 450 → 0): the same changer choice as the
  distance changers; "Camera Offset" `SmoothDamp`s its x (0.2 s), taking the camera along.
* `GroundRaycastCameraInfluencer` (lunar module, while flown): a downward ray from the root on
  layer 0 adds its hit point as a camera influence.
* `ColorCycler` (Corruption 2's "Big Bad"): body colour round the hue circle every 10 s at value
  1 (0.5 for the flash time after each hit), helmet × 0.898.
* `RandomizeChildPositions` (Corruption 2's apartment "Rest" blocks): children swap local
  positions on load, randomly per load (the original seeds from the clock too).
* `TileChild` is an editor-time tool (no runtime caller; the scene already holds its
  `Tile(Clone)` copies); `LocallyDeactivateOffscreen` (Treetops' background tiles, radius 50)
  and `DetachChildWhenDisabled` only cull what is off screen, which the renderer's view test
  already does: nothing observable to port.
Still missing per level (script inventory): Moon's `Dock`.
Reference screenshots of the original (Pirates, Moon, Skyline, Japan, Lobby) are in
`HD GAME SCREENSHOTS/` next to `HD GAME REF/`.


- Effects: `SpawnLocallyOnExplode` prefabs other than particle effects; `ColorCycler` (source missing); `Dock` (Moon's end, with the lunar module).
- HUD: control prompts other than "Enter Vehicle".
- Lobby: touch input; `RemoteLevelAvailability` (levels another machine has unlocked); original Level Loading scene; achievement triggers outside level end (`four frag shot`, `0 shot perfect health …`).
- Level flow: server/client menus; `TimeLimit` finishers are host-only in a network game.
- Vehicles: the lunar module's `Dock`; `VehicleOccupantIK`'s head look-at on gunners; cars (`CarControl`, `WheelJoint2D`), rockets (`RocketControl`), cannons (`CannonControl`) — the contact solver exists now; cars still need Box2D's wheel joint; the "Enter Vehicle" control prompt.
- Weapons: verify Shuriken, Pistol and Spear end to end in Japan / Pirates / Treetops.
- Enemies: cannons; moving platforms.
- Audio: stereo panning.
- Networking: transport is TCP on the LAN and iroh QUIC on the internet, not RakNet (the original cannot join); cheats from the launcher also act on other machines' avatars in a network game; breakable scenery and letters are simulated per machine; helicopter hulls follow their owner by a simple pull (no `SynchronizeRigidBody2D` extrapolation); projectiles are simulated by every machine from the shared input (only the host's hit); not yet tried between two machines.

## 8. Next steps

0. Cars (Moon, Moon Race, Corruption 2): first version in `sticks_game::cars` (spawn from
   `Level::cars`, `CarDrive` + `step_with_joints` per car, boarding/driving/leaving through
   `vehicles`, camera influencers, hulls as obstacles, projectile hits, explosion). Still to do:
   car–car and car–helicopter contacts, the gunner seat (`CarGunnerControl`), the wheels' `PhysicsMaterial2D`, the flip goals, the engine sound, the driver's HUD
   bar showing the car's health, enemies targeting cars; then rockets (`RocketControl`) and
   cannons (`CannonControl`).
1. `cargo check -p sticks_game` / `cargo clippy` on a machine with crates.io (this pass changed input, lobby, HUD, life, vehicles, level loading and added `input.rs` / `versus.rs` / `scenery.rs` without a compiler); then a Versus game in Japan VS with a gamepad + keyboard, and Japan's campaign (sky, clouds, lamps, emerging enemies).
2. Rockets (`RocketControl`) and cannons (`CannonControl`) on top of `sticks_sim::contact`.
3. Run every campaign level to its end (Skyline marker, Office enemy count, Corruption 2 boss) and compare the end-of-level screen with the original; `Emerge`, persistence (`DataManager` scores, tips), Moon Race's time limit, UI-layer level texts.
4. Lobby remainder (Level Loading scene; the in-level achievement triggers).
5. Importer (requested 2026-10-26): players point the game at their own install (`<Game>_Data`, or found in the Steam library folders); on first run the game runs a pinned AssetRipper version headless into a cache folder, checks that the output matches the expected `ExportedProject` layout (script GUIDs, file names), remembers the location, and makes `STICK_EXPORT_ROOT` optional. AssetRipper (GPL-3.0) is downloaded or user-supplied, never bundled. Later option: replace it with a native Rust reader of the Unity 5.1 serialized files.
