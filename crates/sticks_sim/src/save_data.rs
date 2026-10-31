//! `DataManager`'s four save files, byte for byte: `Goals.dat`, `Scores.dat`, `Colors.dat` and
//! `Tips.dat`. The original writes them with .NET's `BinaryWriter` (little-endian integers and
//! floats, one-byte booleans, strings as a 7-bit-encoded UTF-8 byte count and the bytes) into
//! Steam Remote Storage; the same bytes are read and written here, so files copied from the
//! original's Steam cloud folder load as they are.
//!
//! Each file starts with a format version; a file of another version is ignored, as in the
//! original (`Deserialize` returns without reading).

use std::collections::HashMap;

use crate::goals::{GoalProgress, Goals, goal_spec};
use crate::level_end::{HighScores, PlayerId, Score, ScoreTable, TipsSeen};
use crate::lobby::{CharacterColors, ColorAssignments};

const GOAL_DATA_VERSION: i32 = 2;
const SCORE_DATA_VERSION: i32 = 6;
const COLOR_DATA_VERSION: i32 = 1;
const TIPS_DATA_VERSION: i32 = 1;

/// Seconds between 1601-01-01 (Windows file time) and 1970-01-01.
const FILE_TIME_EPOCH_OFFSET: f64 = 11_644_473_600.0;

/// `System.IO.BinaryWriter`.
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn bool(&mut self, v: bool) {
        self.0.push(u8::from(v));
    }

    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    /// `Write(string)`: `Write7BitEncodedInt(byte count)` then UTF-8.
    pub fn string(&mut self, s: &str) {
        let mut n = s.len() as u32;
        while n >= 0x80 {
            self.0.push((n as u8) | 0x80);
            n >>= 7;
        }
        self.0.push(n as u8);
        self.0.extend_from_slice(s.as_bytes());
    }
}

/// `System.IO.BinaryReader`; every read fails past the end of the data.
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, at: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let bytes = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(bytes)
    }

    pub fn bool(&mut self) -> Option<bool> {
        Some(self.take(1)?[0] != 0)
    }

    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn string(&mut self) -> Option<String> {
        let mut n: u32 = 0;
        for shift in (0..35).step_by(7) {
            let b = self.take(1)?[0];
            n |= u32::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                let bytes = self.take(n as usize)?;
                return Some(String::from_utf8_lossy(bytes).into_owned());
            }
        }
        None
    }

    /// A count read as `Int32` (negative counts are corrupt data).
    fn count(&mut self) -> Option<usize> {
        usize::try_from(self.i32()?).ok()
    }
}

fn write_string_or_null(w: &mut Writer, s: Option<&str>) {
    w.bool(s.is_some());
    if let Some(s) = s {
        w.string(s);
    }
}

fn read_string_or_null(r: &mut Reader<'_>) -> Option<Option<String>> {
    Some(if r.bool()? { Some(r.string()?) } else { None })
}

/// `PlayerID.Serialize`: platform, id and last known name, each a nullable string. An empty name
/// stands for a null one.
pub fn write_player_id(w: &mut Writer, id: &PlayerId) {
    write_string_or_null(w, id.platform.as_deref());
    write_string_or_null(w, Some(&id.id));
    write_string_or_null(w, (!id.name.is_empty()).then_some(id.name.as_str()));
}

pub fn read_player_id(r: &mut Reader<'_>) -> Option<PlayerId> {
    let platform = read_string_or_null(r)?;
    let id = read_string_or_null(r)?.unwrap_or_default();
    let name = read_string_or_null(r)?.unwrap_or_default();
    Some(PlayerId { platform, id, name })
}

/// `DateTime.ToFileTimeUtc` of a time in seconds since the Unix epoch.
fn file_time(unix_seconds: f64) -> i64 {
    ((unix_seconds + FILE_TIME_EPOCH_OFFSET) * 10_000_000.0).round() as i64
}

fn unix_seconds(file_time: i64) -> f64 {
    file_time as f64 / 10_000_000.0 - FILE_TIME_EPOCH_OFFSET
}

fn write_score(w: &mut Writer, s: &Score) {
    w.i32(s.value);
    w.i32(s.ids.len() as i32);
    for id in &s.ids {
        write_player_id(w, id);
    }
    w.bool(s.golden);
    w.i64(file_time(s.time));
}

fn read_score(r: &mut Reader<'_>) -> Option<Score> {
    let value = r.i32()?;
    let n = r.count()?;
    let ids = (0..n).map(|_| read_player_id(r)).collect::<Option<Vec<_>>>()?;
    let golden = r.bool()?;
    let time = unix_seconds(r.i64()?);
    Some(Score::new(ids, value, golden, time))
}

/// `HighScores.Serialize` (`Scores.dat`).
pub fn write_scores(scores: &HighScores) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32(SCORE_DATA_VERSION);
    // Sorted for a stable file (the original writes in dictionary order).
    let mut tables: Vec<_> = scores.tables.iter().collect();
    tables.sort_by(|a, b| a.0.cmp(b.0));
    w.i32(tables.len() as i32);
    for ((level, players, difficulty), table) in tables {
        w.string(level);
        w.i32(*players as i32);
        w.i32(*difficulty);
        w.i32(table.scores.len() as i32);
        for s in &table.scores {
            write_score(&mut w, s);
        }
        w.i32(table.best.len() as i32);
        for s in &table.best {
            write_score(&mut w, s);
        }
    }
    w.0
}

/// `HighScores.Deserialize`: tables read replace the ones in memory (others are kept).
pub fn read_scores(data: &[u8], scores: &mut HighScores) -> Option<()> {
    let mut r = Reader::new(data);
    if r.i32()? != SCORE_DATA_VERSION {
        return Some(());
    }
    for _ in 0..r.count()? {
        let level = r.string()?;
        let players = r.count()?;
        let difficulty = r.i32()?;
        let mut table = ScoreTable::default();
        for _ in 0..r.count()? {
            table.scores.push(read_score(&mut r)?);
        }
        for _ in 0..r.count()? {
            table.best.push(read_score(&mut r)?);
        }
        scores.tables.insert((level, players, difficulty), table);
    }
    Some(())
}

fn write_color(w: &mut Writer, c: Option<[f32; 4]>) {
    w.bool(c.is_some());
    if let Some(c) = c {
        for v in c {
            w.f32(v);
        }
    }
}

fn read_color(r: &mut Reader<'_>) -> Option<Option<[f32; 4]>> {
    Some(if r.bool()? { Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?]) } else { None })
}

/// `LocalPlayerColorAssignments.Serialize` (`Colors.dat`).
pub fn write_colors(colors: &ColorAssignments) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32(COLOR_DATA_VERSION);
    w.i32(colors.0.len() as i32);
    for (id, c) in &colors.0 {
        write_player_id(&mut w, id);
        write_color(&mut w, c.body);
        write_color(&mut w, c.helmet);
        write_color(&mut w, c.face);
    }
    w.0
}

/// `LocalPlayerColorAssignments.Deserialize`: replaces every assignment.
pub fn read_colors(data: &[u8], colors: &mut ColorAssignments) -> Option<()> {
    let mut r = Reader::new(data);
    if r.i32()? != COLOR_DATA_VERSION {
        return Some(());
    }
    let n = r.count()?;
    let mut read = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        let id = read_player_id(&mut r)?;
        let c = CharacterColors { body: read_color(&mut r)?, helmet: read_color(&mut r)?, face: read_color(&mut r)? };
        read.push((id, c));
    }
    colors.0 = read;
    Some(())
}

/// `Goals.Serialize` (`Goals.dat`).
pub fn write_goals(goals: &Goals) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32(GOAL_DATA_VERSION);
    w.i32(goals.progress.len() as i32);
    for (id, progress) in &goals.progress {
        write_player_id(&mut w, id);
        let mut entries: Vec<_> = progress.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        w.i32(entries.len() as i32);
        for (goal, p) in entries {
            w.string(goal);
            match p {
                GoalProgress::Standard { done } => w.bool(*done),
                GoalProgress::AllLevel { levels } => {
                    w.i32(levels.len() as i32);
                    for l in levels {
                        w.string(l);
                    }
                }
            }
        }
    }
    w.0
}

/// `Goals.Deserialize`: replaces all progress. A goal id this version does not know is read as
/// a standard goal and dropped, as in the original.
pub fn read_goals(data: &[u8], goals: &mut Goals) -> Option<()> {
    let mut r = Reader::new(data);
    if r.i32()? != GOAL_DATA_VERSION {
        return Some(());
    }
    let mut progress = Vec::new();
    for _ in 0..r.count()? {
        let id = read_player_id(&mut r)?;
        let mut map = HashMap::new();
        for _ in 0..r.count()? {
            let goal = r.string()?;
            match goal_spec(&goal) {
                Some(spec) if spec.all_level => {
                    let levels = (0..r.count()?).map(|_| r.string()).collect::<Option<Vec<_>>>()?;
                    map.insert(goal, GoalProgress::AllLevel { levels });
                }
                Some(_) => {
                    let done = r.bool()?;
                    map.insert(goal, GoalProgress::Standard { done });
                }
                None => {
                    r.bool()?;
                }
            }
        }
        progress.push((id, map));
    }
    goals.progress = progress;
    Some(())
}

/// `LevelTips.Serialize` (`Tips.dat`).
pub fn write_tips(tips: &TipsSeen) -> Vec<u8> {
    let mut w = Writer::default();
    w.i32(TIPS_DATA_VERSION);
    let mut entries: Vec<_> = tips.seen.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    w.i32(entries.len() as i32);
    for ((level, control_types), players) in entries {
        w.string(level);
        w.i32(control_types.len() as i32);
        for c in control_types {
            w.string(c);
        }
        w.i32(players.len() as i32);
        for p in players {
            write_player_id(&mut w, p);
        }
    }
    w.0
}

/// `LevelTips.Deserialize`: replaces everything seen.
pub fn read_tips(data: &[u8], tips: &mut TipsSeen) -> Option<()> {
    let mut r = Reader::new(data);
    if r.i32()? != TIPS_DATA_VERSION {
        return Some(());
    }
    let mut seen = HashMap::new();
    for _ in 0..r.count()? {
        let level = r.string()?;
        let control_types = (0..r.count()?).map(|_| r.string()).collect::<Option<Vec<_>>>()?;
        let players = (0..r.count()?).map(|_| read_player_id(&mut r)).collect::<Option<Vec<_>>>()?;
        seen.insert((level, control_types), players);
    }
    tips.seen = seen;
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotnet_strings() {
        let mut w = Writer::default();
        w.string("Skyline");
        assert_eq!(w.0[0], 7);
        let long = "x".repeat(200);
        let mut w2 = Writer::default();
        w2.string(&long);
        assert_eq!(&w2.0[..2], &[0xc8, 0x01]);
        assert_eq!(Reader::new(&w2.0).string().as_deref(), Some(long.as_str()));
        assert_eq!(Reader::new(&[5, b'a']).string(), None);
    }

    #[test]
    fn scores_round_trip() {
        let mut scores = HighScores::default();
        let me = PlayerId::anonymous_local(0);
        scores.table("Skyline", 1, 0).add(Score::new(vec![me.clone()], 4564, false, 1_790_000_000.0));
        scores.table("Japan", 1, -1).add(Score::new(vec![me], 9000, true, 1_790_000_100.0));
        let bytes = write_scores(&scores);
        let mut back = HighScores::default();
        read_scores(&bytes, &mut back).unwrap();
        assert_eq!(back.table("Skyline", 1, 0).scores()[0].value, 4564);
        assert!(back.table("Japan", 1, -1).scores()[0].golden);
        assert!((back.table("Japan", 1, -1).scores()[0].time - 1_790_000_100.0).abs() < 1e-3);
        // Another version is ignored.
        let mut other = HighScores::default();
        read_scores(&[7, 0, 0, 0], &mut other).unwrap();
        assert_eq!(other, HighScores::default());
    }

    #[test]
    fn colours_goals_and_tips_round_trip() {
        let me = PlayerId::anonymous_local(0);
        let mut colors = ColorAssignments::default();
        colors.set(&me, CharacterColors { body: Some([1.0, 0.5, 0.25, 1.0]), helmet: None, face: None });
        let mut back = ColorAssignments::default();
        read_colors(&write_colors(&colors), &mut back).unwrap();
        assert_eq!(back.get(&me).body, Some([1.0, 0.5, 0.25, 1.0]));

        let mut goals = Goals::default();
        goals.goal_done(&me, "complete -1", Some("Skyline"));
        goals.goal_done(&me, "rover 720", None);
        let mut goals_back = Goals::default();
        read_goals(&write_goals(&goals), &mut goals_back).unwrap();
        assert_eq!(goals_back.done_count(&me, 1), 2);

        let mut tips = TipsSeen::default();
        assert!(tips.mark("Japan", &["Keyboard".to_owned()], std::slice::from_ref(&me)));
        let mut tips_back = TipsSeen::default();
        read_tips(&write_tips(&tips), &mut tips_back).unwrap();
        assert!(!tips_back.mark("Japan", &["Keyboard".to_owned()], &[me]));
    }
}
