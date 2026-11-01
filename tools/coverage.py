#!/usr/bin/env python3
"""How much of the original game's code the port covers, as a treemap and a per-level table.

Every method of the decompiled game (`source/Assembly-CSharp`, one square per method, sized by
its lines) is classified by what the Rust code says about it:

- ported and checked: named in the Rust code (`Class.Method`, or the class plus a Rust item with
  the method's snake_case name, or the class's Unity messages such as `Update`), and the class
  appears in a test;
- ported: named, but in no test;
- to port: neither;
- nothing to port: platform services, third-party plumbing and engine glue the port replaces
  (the `NOT_APPLICABLE` list below);
- unused: a component no scene or prefab of the game uses.

Outputs (all next to each other in --out):
  coverage.svg        the treemap (README and progress.html embed it)
  coverage.json       the numbers, per system, class and level
  levels.html         a table: per level, how much of the code its scenes use is ported

With --progress FILE the treemap and the level table also replace the "Original code covered"
section of that progress page (between its coverage markers; added before the subsystem audit
the first time).

The classification reads names, so it is an estimate: a method the port reproduces under another
name counts as still to port, and a mention is not proof of fidelity (IMPLEMENTATION_MAP.md has
the audited state).

Usage:
  python tools/coverage.py --source ../source/Assembly-CSharp \
      --assets ../assets_exported/ExportedProject/Assets --crates crates --out docs
"""

import argparse
import datetime
import html
import json
import os
import re
from collections import defaultdict

# --- CONFIGURABLE RESOLUTION & LAYOUT SETTINGS ---
MAP_WIDTH = 2000
MAP_HEIGHT = 1440
HEAD_HEIGHT = 32
TOP_OFFSET = 148
# -----------------------------------------------

# Engine, platform and third-party plumbing with nothing to port: the port has its own
# equivalents (Bevy, gilrs, ECS pools) or the feature does not exist on this platform.
NOT_APPLICABLE = {
    "SteamManager": "Steamworks.NET wrapper",
    "SteamOverlay": "Steam overlay pause",
    "SteamUpgradeHandler": "Starter Edition upgrade through Steam",
    "SocialManager": "Game Center / Steam achievements",
    "UpgradeButton": "Starter Edition upgrade",
    "WatchTrailerButton": "store trailer link",
    "ReturnFromStarterEditionRestrictionsScreenIfUpgraded": "Starter Edition",
    "MuteMusicWhenPlatformMusicIsPlaying": "iOS music app",
    "MuteMusicInDevelopment": "editor helper",
    "TestIK": "editor test",
    "TestIKLookAt": "editor test",
    "InControlController": "InControl input library (Bevy/gilrs here)",
    "XInputController": "XInput library (Bevy/gilrs here)",
    "Pool": "object pool (ECS spawning here)",
    "NetworkPool": "network object pool (ECS spawning here)",
    "Prepooler": "object pool warm-up",
    "PausableCoroutineRunner": "Unity coroutine helper",
    "UpdateWhenDisabledHelper": "Unity update helper",
    "SetMinimumAllocatableNetworkViewIDs": "RakNet view ids",
    "SetNetworkViewGroup": "RakNet view groups",
    "ScaleWithPhysicalScreenHeight": "phone screens",
    "BigScreenOrthographicCameraScaler": "TV screens (Ouya)",
}

# Systems, first match wins (class names).
SYSTEMS = [
    ("network", r"Network|Server|Client|Remote|UniqueClient|OwnerOnly|ServerOnly|SyncServer|SynchronizeRigid|SendExplode|TellOwner|Disconnect|ConnectionLimit|PlayerSlotAssignments|RemoveDisconnected|RemoveOwnerAndServer|EnableBehaviourWhenOffline|TakeResponsibility|LocalPlayerColor"),
    ("input", r"Input|Gamepad|Keyboard|^Mouse|Joystick|Controller$|Processor|Prioritizer|ControlTypes|LayoutControls|ControlPrompt|AllClientControlTypes"),
    ("vehicles", r"Helicopter|^Car|Cannon|Rocket|Vehicle|ControlSlot|Passenger|^Dock|Gunner|Eject|MoveToOtherControlSlot|LinkBehaviourEnablementToControlSlot"),
    ("enemies & AI", r"Enemy|^Wave|Emerge|MoveToFirstTarget|FireAtFirstTarget|FireAndMove|IsRootControlledTarget|IdleControlling|Whale|Seeker"),
    ("weapons & damage", r"Weapon|Projectile|Damage|Charge|Explo|Health|Launcher|Diffusion|StarShower|MaximumDamage|DontExplode|SetProjectileSpeed"),
    ("level flow & goals", r"Level|Goal|Ender|Trigger|TimeLimit|Lives|Respawn|Spawn|Score|Versus|EndOf|GameOver|Difficulty|MinimumPlayers|Unsafe|StopRespawning|Activate|DestroyAll|DestroyParticular|AllowedWeapons|GravityChanger|FourFrag|ZeroShot|HighScores|DataManager|Unlock|Ending|Achievement"),
    ("lobby & menus", r"Lobby|Button|Menu|Select|Customize|Color|Team|GameSummary|Splash|Draggable|Exclusively|Error|Version|WebLink|Trailer|Upgrade|Starter|NoControllers|Slider|Highlight|Changer|Pause|ControlAssignment|TrivialFPS|SocialManager|Steam"),
    ("HUD", r"UI$|UIActivator|Display|Indicator|^Pointer|Speedometer|Tips|Prompt|HidePlayerInfo|Table|Timer|ViewportAnchor|Screen|Scaler|AspectRatio"),
    ("camera", r"Camera"),
    ("audio", r"Sound|Music|Audio"),
    ("characters", r"Character|Player|Duck|IK|Aim|Occupant|Gravity"),
    ("rendering & effects", r"Sprite|Shadow|Lines|Render|Particle|Dots|tk2d|Text|Flat|Scale|Fade|Rotate|Follow|Copy|Despawn|Destroy|Lifetime|Scroller|Fix|Detach|Diffusion"),
]

LIFECYCLE = {
    "Awake", "Start", "Update", "FixedUpdate", "LateUpdate", "OnEnable", "OnDisable", "OnDestroy",
    "OnGUI", "OnTriggerEnter2D", "OnTriggerExit2D", "OnTriggerStay2D", "OnCollisionEnter2D",
    "OnCollisionExit2D", "OnCollisionStay2D", "OnMouseEnter", "OnMouseExit", "OnMouseDown",
    "OnMouseUp", "OnMouseUpAsButton", "OnAnimatorMove", "OnAnimatorIK", "OnLevelWasLoaded",
    "Reset", "OnValidate", "OnBecameVisible", "OnBecameInvisible",
}

STATUS = ["checked", "ported", "to port", "nothing to port", "unused"]
COLORS = {"checked": "#1f7a35", "ported": "#46be63", "to port": "#2f6fdf", "nothing to port": "#e5484d", "unused": "#4a4c54"}
LABELS = {
    "checked": "ported and checked",
    "ported": "ported",
    "to port": "still to port",
    "nothing to port": "nothing to port (replaced by ours)",
    "unused": "not used by the game",
}

MEMBER = re.compile(
    r"^\s*(?:\[[^\]]*\]\s*)*(?:(?:public|private|protected|internal|static|virtual|override|abstract|sealed|new|extern|unsafe|async|implicit|explicit|operator)\s+)*"
    r"(?:[\w<>\[\],\.\?]+\s+)?(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)?"
)
TYPE_DECL = re.compile(r"\b(class|struct|interface|enum)\s+(\w+)")
KEYWORDS = {"if", "for", "foreach", "while", "switch", "using", "lock", "catch", "return", "new", "base", "this", "get", "set", "add", "remove", "delegate", "typeof", "sizeof", "nameof", "else", "do", "try", "fixed", "checked"}


def snake(name):
    s = re.sub(r"(?<=[a-z0-9])([A-Z])", r"_\1", name)
    s = re.sub(r"(?<=[A-Z])([A-Z][a-z])", r"_\1", s)
    return s.lower()


def strip_strings(line):
    line = re.sub(r'"(?:\\.|[^"\\])*"', '""', line)
    line = re.sub(r"'(?:\\.|[^'\\])'", "''", line)
    return line.split("//")[0]


def parse_cs(path):
    """(top-level class, method, lines) for every method, constructor and accessor body."""
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    out = []
    depth = 0
    types = []  # (name, depth inside the type body)
    pending_type = None
    member = None  # (name, start line, depth at which its body opens)
    for i, raw in enumerate(lines):
        line = strip_strings(raw)
        t = TYPE_DECL.search(line)
        if t and member is None:
            pending_type = t.group(2)
        elif member is None and types and depth == types[-1][1]:
            m = MEMBER.match(line)
            if m and m.group(1) not in KEYWORDS and not line.rstrip().endswith(";"):
                member = (m.group(1), i, depth)
        for ch in line:
            if ch == "{":
                depth += 1
                if pending_type is not None:
                    types.append((pending_type, depth))
                    pending_type = None
            elif ch == "}":
                if member is not None and depth == member[2] + 1:
                    out.append((types[0][0], member[0], i - member[1] + 1))
                    member = None
                if types and depth == types[-1][1]:
                    types.pop()
                depth -= 1
        if member is not None and line.rstrip().endswith(";") and depth == member[2]:
            out.append((types[0][0] if types else member[0], member[0], 1))
            member = None
    return out


def system_of(cls):
    for name, pattern in SYSTEMS:
        if re.search(pattern, cls):
            return name
    return "core & helpers"


def scan_rust(crates):
    texts = {}
    for root, _, files in os.walk(crates):
        if "target" in root.split(os.sep):
            continue
        for f in files:
            if f.endswith(".rs"):
                p = os.path.join(root, f)
                with open(p, encoding="utf-8", errors="replace") as fh:
                    texts[p] = fh.read()
    return texts


def script_guids(assets):
    guids = {}
    for root, _, files in os.walk(os.path.join(assets, "Scripts")):
        for f in files:
            if f.endswith(".cs.meta"):
                with open(os.path.join(root, f), encoding="utf-8", errors="replace") as fh:
                    m = re.search(r"guid: (\w+)", fh.read())
                if m:
                    guids[m.group(1)] = f[: -len(".cs.meta")]
    return guids


def guids_in(path):
    with open(path, encoding="utf-8", errors="replace") as fh:
        return set(re.findall(r"guid: ([0-9a-f]{32})", fh.read()))


def squarify(items, x, y, w, h):
    """Squarified treemap: items are (value, payload); returns (x, y, w, h, payload)."""
    items = [it for it in items if it[0] > 0]
    total = sum(v for v, _ in items)
    if not items or w <= 0 or h <= 0 or total <= 0:
        return []
    scale = w * h / total
    rest = [(v * scale, p) for v, p in sorted(items, key=lambda t: -t[0])]
    out = []
    while rest:
        short = min(w, h)
        row = [rest[0]]
        def worst(r):
            s = sum(a for a, _ in r)
            return max(max(short * short * a / (s * s), (s * s) / (short * short * a)) for a, _ in r)
        k = 1
        while k < len(rest) and worst(row + [rest[k]]) <= worst(row):
            row.append(rest[k])
            k += 1
        rest = rest[k:]
        s = sum(a for a, _ in row)
        if w >= h:
            col = s / h
            yy = y
            for a, p in row:
                hh = a / col
                out.append((x, yy, col, hh, p))
                yy += hh
            x += col
            w -= col
        else:
            rw = s / w
            xx = x
            for a, p in row:
                ww = a / rw
                out.append((xx, y, ww, rw, p))
                xx += ww
            y += rw
            h -= rw
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", required=True)
    ap.add_argument("--assets", required=True)
    ap.add_argument("--crates", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--progress", help="progress.html to update")
    args = ap.parse_args()

    methods = []
    for root, _, files in os.walk(args.source):
        for f in sorted(files):
            if f.endswith(".cs"):
                for cls, name, n in parse_cs(os.path.join(root, f)):
                    methods.append({"class": cls, "method": name, "lines": max(n, 1)})

    rust = scan_rust(args.crates)
    all_text = "\n".join(rust.values())
    test_text = "\n".join(
        t if (os.sep + "tests" + os.sep) in p else t.split("#[cfg(test)]", 1)[1] if "#[cfg(test)]" in t else "" for p, t in rust.items()
    )
    classes = sorted({m["class"] for m in methods})
    class_files = {c: [t for t in rust.values() if re.search(r"(?<![\w])" + re.escape(c) + r"(?![\w])", t)] for c in classes}
    tested = {c for c in classes if re.search(r"(?<![\w])" + re.escape(c) + r"(?![\w])", test_text)}

    guid_class = script_guids(args.assets)
    scenes = []
    for root, _, files in os.walk(os.path.join(args.assets, "Game")):
        for f in files:
            if f.endswith(".unity"):
                scenes.append(os.path.join(root, f))
                
    prefabs = {}
    for root, _, files in os.walk(os.path.join(args.assets, "GameObject")):
        for f in files:
            if f.endswith(".prefab"):
                p = os.path.join(root, f)
                meta_path = p + ".meta"
                meta_content = ""
                if os.path.exists(meta_path):
                    with open(meta_path, encoding="utf-8", errors="replace") as fh:
                        meta_content = fh.read()
                m = re.search(r"guid: (\w+)", meta_content)
                prefabs[m.group(1) if m else p] = guids_in(p)

    def used_by(scene):
        seen, todo = set(), set(guids_in(scene))
        while todo:
            g = todo.pop()
            if g in seen:
                continue
            seen.add(g)
            todo |= prefabs.get(g, set()) - seen
        return {guid_class[g] for g in seen if g in guid_class}

    level_use = {os.path.splitext(os.path.basename(s))[0]: used_by(s) for s in sorted(scenes)}
    used_anywhere = set().union(*level_use.values()) if level_use else set()
    for p, refs in prefabs.items():
        used_anywhere |= {guid_class[g] for g in refs if g in guid_class}

    sources = {}
    for root, _, files in os.walk(args.source):
        for f in files:
            if f.endswith(".cs"):
                with open(os.path.join(root, f), encoding="utf-8", errors="replace") as fh:
                    sources[f[:-3]] = fh.read()
    components = {c for c, s in sources.items() if re.search(r":\s*(MonoBehaviour|SubsumableBehavior|Explodable)\b", s)}

    for m in methods:
        c, name = m["class"], m["method"]
        if c in NOT_APPLICABLE:
            status = "nothing to port"
        else:
            exact = re.search(re.escape(c) + r"\." + re.escape(name) + r"(?![\w])", all_text) is not None
            mentioned = bool(class_files[c])
            snake_hit = mentioned and any(re.search(r"(?<![\w])" + re.escape(snake(name)) + r"(?![\w])", t) for t in class_files[c])
            ported = exact or (mentioned and (name in LIFECYCLE or snake_hit or name == c))
            if ported:
                status = "checked" if c in tested else "ported"
            elif c in components and c not in used_anywhere:
                status = "unused"
            else:
                status = "to port"
        m["status"] = status
        m["system"] = system_of(c)

    os.makedirs(args.out, exist_ok=True)
    counts = {s: sum(1 for m in methods if m["status"] == s) for s in STATUS}
    lines = {s: sum(m["lines"] for m in methods if m["status"] == s) for s in STATUS}
    relevant = [m for m in methods if m["status"] in ("checked", "ported", "to port")]
    done = [m for m in relevant if m["status"] in ("checked", "ported")]
    pct = 100.0 * sum(m["lines"] for m in done) / max(1, sum(m["lines"] for m in relevant))

    # Treemap using global config resolutions
    W, H, HEAD, TOP = MAP_WIDTH, MAP_HEIGHT, HEAD_HEIGHT, TOP_OFFSET
    systems = defaultdict(list)
    for m in methods:
        systems[m["system"]].append(m)
    order = {s: i for i, s in enumerate(STATUS)}
    group_rects = squarify([(sum(x["lines"] for x in ms), name) for name, ms in systems.items()], 0, TOP, W, H - TOP)
    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="Segoe UI, system-ui, sans-serif">',
        f'<rect width="{W}" height="{H}" fill="#101216"/>',
        f'<text x="12" y="26" fill="#f2f2f2" font-size="19" font-weight="700">{pct:.1f}% of the original game\'s code ported to Rust ({len(done)} of {len(relevant)} methods)</text>',
        f'<text x="12" y="45" fill="#a6a8ad" font-size="12.5">{counts["checked"]} checked by tests · {counts["to port"]} still to port · {counts["nothing to port"]} nothing to port · {counts["unused"]} not used by the game · one square per method, as big as its code · {datetime.date.today().isoformat()}</text>',
    ]
    lx = 12
    for s in STATUS:
        svg.append(f'<rect x="{lx}" y="54" width="11" height="11" fill="{COLORS[s]}"/>')
        svg.append(f'<text x="{lx + 15}" y="64" fill="#d0d2d6" font-size="12">{html.escape(LABELS[s])}</text>')
        lx += 22 + int(6.6 * len(LABELS[s]))
    for gx, gy, gw, gh, name in group_rects:
        ms = systems[name]
        share = sum(x["lines"] for x in ms if x["status"] in ("checked", "ported"))
        total = sum(x["lines"] for x in ms if x["status"] in ("checked", "ported", "to port"))
        gp = 100.0 * share / total if total else 100.0
        svg.append(f'<rect x="{gx:.1f}" y="{gy:.1f}" width="{gw:.1f}" height="{gh:.1f}" fill="#101216"/>')
        label = f"{name} {gp:.0f}%"
        svg.append(f'<text x="{gx + 3:.1f}" y="{gy + 12:.1f}" fill="#f2f2f2" font-size="11" font-weight="700">{html.escape(label if gw > 7 * len(label) else name[: max(1, int(gw / 7))])}</text>')
        inner = sorted(ms, key=lambda x: (order[x["status"]], -x["lines"]))
        blocks = defaultdict(list)
        for x in inner:
            blocks[x["status"]].append(x)
        block_rects = squarify([(sum(x["lines"] for x in b), s) for s, b in blocks.items()], gx + 1, gy + HEAD, gw - 2, gh - HEAD - 1)
        for bx, by, bw, bh, s in block_rects:
            for rx, ry, rw, rh, x in squarify([(x["lines"], x) for x in blocks[s]], bx, by, bw, bh):
                tip = f'{x["class"]}.{x["method"]} ({x["lines"]} lines): {LABELS[x["status"]]}'
                svg.append(
                    f'<rect x="{rx:.1f}" y="{ry:.1f}" width="{max(rw - 0.6, 0.3):.1f}" height="{max(rh - 0.6, 0.3):.1f}" fill="{COLORS[x["status"]]}" stroke="#101216" stroke-width="0.6"><title>{html.escape(tip)}</title></rect>'
                )
    svg.append("</svg>")
    with open(os.path.join(args.out, "coverage.svg"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(svg) + "\n")

    # Per class and per level
    per_class = defaultdict(lambda: {"lines": 0, "done": 0, "status": defaultdict(int)})
    for m in methods:
        pc = per_class[m["class"]]
        pc["lines"] += m["lines"]
        pc["status"][m["status"]] += m["lines"]
        if m["status"] in ("checked", "ported"):
            pc["done"] += m["lines"]
    rows = []
    for level, used in level_use.items():
        considered = [c for c in used if c in per_class and c not in NOT_APPLICABLE]
        total = sum(per_class[c]["lines"] - per_class[c]["status"]["nothing to port"] for c in considered)
        done_lines = sum(per_class[c]["done"] for c in considered)
        missing = sorted(
            ((c, per_class[c]["lines"] - per_class[c]["done"] - per_class[c]["status"]["nothing to port"]) for c in considered),
            key=lambda t: -t[1],
        )
        missing = [(c, n) for c, n in missing if n > 0]
        rows.append((level, len(considered), 100.0 * done_lines / max(1, total), missing))
    table = ['<table class="levels"><thead><tr><th>Scene</th><th>Scripts used</th><th>Ported</th><th>Largest gaps (lines still to port)</th></tr></thead><tbody>']
    for level, n, p, missing in rows:
        gaps = ", ".join(f"<code>{html.escape(c)}</code> {k}" for c, k in missing[:8]) or "none"
        table.append(f"<tr><td>{html.escape(level)}</td><td>{n}</td><td>{p:.0f}%</td><td>{gaps}</td></tr>")
    table.append("</tbody></table>")
    with open(os.path.join(args.out, "levels.html"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(table) + "\n")

    if args.progress:
        update_progress(args.progress, "\n".join(svg), "\n".join(table), pct, len(done), len(relevant), MAP_WIDTH, MAP_HEIGHT)

    with open(os.path.join(args.out, "coverage.json"), "w", encoding="utf-8", newline="\n") as fh:
        json.dump(
            {
                "date": datetime.date.today().isoformat(),
                "percent_ported": round(pct, 1),
                "methods": counts,
                "lines": lines,
                "systems": {
                    name: {s: sum(x["lines"] for x in ms if x["status"] == s) for s in STATUS} for name, ms in sorted(systems.items())
                },
                "levels": {level: {"scripts": n, "percent": round(p, 1), "gaps": missing[:20]} for level, n, p, missing in rows},
                "classes": {c: {"lines": v["lines"], "done": v["done"], **{s: v["status"][s] for s in STATUS}} for c, v in sorted(per_class.items())},
            },
            fh,
            indent=1,
        )
    print(f"{pct:.1f}% ported ({len(done)} of {len(relevant)} methods); {counts}")
    for level, n, p, missing in rows:
        print(f"  {level:<20} {n:>3} scripts {p:5.1f}%  gaps: {', '.join(f'{c} {k}' for c, k in missing[:6])}")


START, END = "<!-- coverage:start -->", "<!-- coverage:end -->"


def update_progress(path, svg, table, pct, done, relevant, width, height):
    with open(path, encoding="utf-8") as fh:
        page = fh.read()
    svg = svg.replace(f'width="{width}" height="{height}"', 'style="width:100%;height:auto;display:block;border-radius:8px" role="img" aria-label="Treemap of the original game\'s methods by port status"', 1)
    section = f"""{START}
  <section class="card" aria-labelledby="cov">
    <h2 id="cov">Original code covered</h2>
    <p class="sub" style="margin-bottom:12px">Every method of the decompiled game, one square each, as big as its code, grouped by system: {pct:.1f}% of the code ({done} of {relevant} methods) is named by the port. Hover a square for the method. Generated by <code>tools/coverage.py</code> from names, so it is an estimate; the audit below is the checked state.</p>
    {svg}
    <h2 style="margin-top:20px">Levels</h2>
    <p class="sub" style="margin-bottom:12px">The scripts each scene (and the prefabs it places) uses, and how much of their code is ported. The gaps are what stands between a level and the original's behaviour.</p>
    <div class="table-scroll">
    {table}
    </div>
  </section>
  {END}"""
    if START in page and END in page:
        a, b = page.index(START), page.index(END) + len(END)
        page = page[:a] + section + page[b:]
    else:
        anchor = '<section class="card" aria-labelledby="audit">'
        page = page.replace(anchor, section + "\n\n  " + anchor, 1) if anchor in page else page.replace("</main>", section + "\n</main>", 1)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(page)


if __name__ == "__main__":
    main()