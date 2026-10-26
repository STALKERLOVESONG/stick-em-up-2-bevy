# Lobby evidence

Primary serialized source: `assets_exported/ExportedProject/Assets/Game/Lobby/Lobby.unity`.
The build transition evidence is in `bootstrap_evidence.md`.

## Root hierarchy recovered

The scene has these root GameObjects (all serialized `m_IsActive: 1`):
`Level Music`, `Helmet Model 0..3`, `Face Model 0..3`, `Body Model 0..3`,
`Summary Model 0..3`, `Lobby Client Disconnection`, `Mode Select Screen`,
`Control Assignment Screen`, `Credits Screen 1`, `Game Summary Screen`,
`Allowed Weapons`, `Splash Screen`, `Error Screen`, `Lobby Input Manager`,
`Team Arrangement Screen`, `Customize Body Color Screen`, `Make Game Mode
Consistent with Player Count`, `Credits Screen 2`, `Background`,
`Achievements Screen 1..3`, `Customize Face Color Screen`, `Customize Helmet
Color Screen`, `Server List`, `Unconditional Mouse Needer`, `Camera`, `No
Controllers Screen`, `Level Select Screen`, `Starter Edition Restrictions
Screen`, `Server Broadcaster`, `High Scores Screen`, `Nearly Empty Scene
Loader`, `Title Screen`, and `Permanent System Loader`.

Important serialized child/root facts: `Splash` is serialized inactive;
`Network Setup` is serialized inactive; `Camera` local position is
`(-54.42, 50.72, -40.57)`; `CameraPositioner` has margin 25, minimum distance
23.7, maximum distance 70, and `smoothTimeZ` 0.2. The scene is built with
Transform, Camera, tk2d sprites/text/renderers, colliders, and MonoBehaviours;
it is not a Canvas/EventSystem hierarchy.

## Resolved script mappings and behavior

- `LobbyStartupDirector.Start` activates splash on first entry; on subsequent
  entry it selects summary, network setup, or the disconnect error based on
  `ClientDisconnection` state. Source: `LobbyStartupDirector.cs`.
- `PermanentSystemLoader.Awake` additive-loads Permanent System only when
  `Utility.PermanentSystem` is absent, then destroys itself. Source:
  `PermanentSystemLoader.cs`.
- `LobbyInputManager.OnEnable` starts a coroutine; after two rendered frames it
  creates persistent `Lobby Inputs` from Permanent System processors, then
  sets `Ready` one frame later. `OnDisable` deactivates its first input object.
- `ActivateExclusively` keeps a static instance list and, on enable, activates
  only itself. This is the screen-state mechanism, not generic menu routing.
- `SplashScreenAdvance.Update` reacts to any lobby input or `timeElapsed > 3f`.
  Keyboard input is left/right/Escape/anything-pressed only; select and
  up/down are false. Source: `KeyboardAndMouseLobbyInput.cs`.
- `Button` uses gamepad-only highlighted navigation (`MoveUp/Down/Left/Right`,
  then Select), or mouse/tk2dUI callbacks. Escape drives visible `BackButton`.
- `MakeGameModeConsistentWithPlayerCount.Update` demotes TeamVersus below 3
  players and Versus below 2 players, only offline/server-side.

Resolved YAML GUID mappings include `LobbyClientDisconnection`,
`AllowedWeapons`, `LobbyInputManager`, `LinesRendererManager`,
`MakeGameModeConsistentWithPlayerCount`, `LobbyStartupDirector`,
`UnconditionalMouseNeeder`, `ServerBroadcaster`, `LoadNearlyEmptyScene`,
`PermanentSystemLoader`, `CameraPositioner`, `BigScreenCameraDistancer`, and
`EventSounds`. Their `.meta` mappings and the corresponding source files are
under `assets_exported/ExportedProject/Assets/Scripts/Assembly-CSharp` and
`source/Assembly-CSharp`.

## Renderer/assets

The current software renderer cannot render tk2d sprite collections, text
meshes, animation clips, MasterAudio events, or Unity materials. No Lobby
asset is loaded as a substitute; recovered component names are retained in the
world model instead. Implement a tk2d-compatible data/renderer path only after
its serialized references and draw order are traced.

## Uncertainties

TODO(UNCERTAIN): full child hierarchy, every text string/button link, and
`ActivateExclusively` final ordering require a complete serialized-reference
resolver plus executable trace. They are not inferred from root names. The
current state preserves recovered roots and script attachment only; it does
not claim visual or interaction equivalence yet.
