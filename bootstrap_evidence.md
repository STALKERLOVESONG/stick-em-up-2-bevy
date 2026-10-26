# Recovered bootstrap evidence

This report lives under `reconstruction/` rather than `analysis/`: project
instructions mark `analysis/` read-only.

## Probable entry and order

`assets_exported/ExportedProject/ProjectSettings/EditorBuildSettings.asset`
places **Permanent System** at build index 0 and **Lobby** at index 1.
`LobbyLoader.Start` (`source/Assembly-CSharp/LobbyLoader.cs`) runs in index 0:

1. If `Application.loadedLevel == 0`, yield `null` exactly 30 times.
2. `Application.LoadLevel(1)`.
3. Destroy the loader GameObject.

Therefore the first meaningful interactive state is **Lobby**, reached from
the **Permanent System** bootstrap scene after 30 rendered-frame yields. The
Rust `Bootstrap` retains that exact counter and does not substitute a seconds
timer.

## Persistent and global objects

The serialized `Permanent System.unity` includes named active objects: `Steam
Manager`, `Resolution Manager`, `Goals`, `Data Manager`, `Level Loader`,
`Lobby Loader`, `In-Game UI`, and `UI Camera`, under `Permanent System`.
`Utility.PermanentSystem` finds `/Permanent System`; the `DontDestroyOnLoad`
component calls Unity's persistence API in `DontDestroyOnLoad.cs`.

Relevant source behavior:

- `SteamManager.Start`: singleton, persistent, waits for
  `ResolutionManager.StartupComplete`, then initializes Steam.
- `Goals.Start`: yields one frame before building goal state and assigning
  `Goals.Instance`.
- `DataManager.Start`: waits for `Goals.Instance` then `SteamManager.Initialized`
  before loading `Goals.dat`, `Scores.dat`, `Colors.dat`, and `Tips.dat`.
- `LevelLoader.Start`: assigns its singleton and `NetworkView`.
- `LobbyInputManager.Run`: waits two frames, then creates persistent
  `Lobby Inputs` if missing.

## Lobby and later transitions

`PermanentSystemLoader.Awake` loads **Permanent System** additively only when
`Utility.PermanentSystem` is null, then destroys itself. The Lobby scene
contains `Startup Director`, `Splash Screen`, inactive `Splash`, `Lobby Input
Manager`, inactive `Network Setup`, and `Permanent System Loader` according to
the serialized YAML. `LobbyStartupDirector.Start` selects splash on first
visit, network setup after deliberate disconnect, the error screen after an
accidental disconnect, and summary after later visits.

`LevelLoader` transitions through **Level Loading**, persists static mode,
difficulty, and player count, unloads unused resources, asynchronous-loads the
target scene, and broadcasts `OnLoadedLevelEverywhere` after all peers finish.

## Timing evidence

`ProjectSettings/TimeManager.asset` specifies `Fixed Timestep: 0.016666668`
and `Maximum Allowed Timestep: 0.33333334`. `SplashScreenAdvance.Update`
accumulates `Time.deltaTime`; `DataManager` saves from `LateUpdate`.

## Implemented compatibility surface

The present implementation has GameObject identity, named component attachment,
Transform parent links and local values, `activeSelf`/active-hierarchy checks,
Awake/OnEnable/Start/Update/FixedUpdate/LateUpdate/OnDisable/OnDestroy event
records, and deferred destruction. It uses the recovered fixed timestep and
maximum frame delta. It models the recovered bootstrap names and its 30-frame
transition to Lobby. It does not implement any gameplay component behavior.

## Uncertainties

- TODO(UNCERTAIN): exact Unity callback order across unrelated loaded objects
  has not yet been captured from the original executable; creation order is
  preserved until such a trace is available.
- TODO(UNCERTAIN): this phase represents the confirmed initial named objects,
  not every serialized Lobby child or every component reference.
- TODO(UNCERTAIN): persistence and additive scene unloading are represented by
  bootstrap state only; scene-unload semantics require a runtime trace before
  they are generalized.
