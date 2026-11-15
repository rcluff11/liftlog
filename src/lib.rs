//! Shared database + training logic for liftlog.
//!
//! Everything here returns plain data. The CLI (src/main.rs) prints it and the
//! web server (src/bin/server.rs) turns it into JSON.

use chrono::{Days, NaiveDate};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs;
use std::path::PathBuf;

pub const MIN_REPS: u32 = 3;
pub const MAX_REPS: u32 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Set {
    pub weight: f64,
    pub reps: u32,
}

/// What `next` tells you to do with the weight.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Increase,
    Repeat,
    Decrease,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub exercise: String,
    pub last_weight: f64,
    pub working_sets: usize,
    pub fewest_reps: u32,
    pub next_weight: f64,
    pub target_reps: u32,
    pub action: Action,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pr {
    pub exercise: String,
    pub weight: f64,
    pub reps: u32,
    pub date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryDay {
    pub date: String,
    pub sets: Vec<Set>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    pub date: String,
    pub e1rm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutStatus {
    Warning,
    SlightDip,
    Holding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CutCheck {
    pub recent_peak: f64,
    pub change_pct: f64,
    pub status: CutStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trend {
    pub exercise: String,
    pub points: Vec<TrendPoint>,
    /// None when there is no earlier session within 14 days to compare against.
    pub cut_check: Option<CutCheck>,
}

// ---------- database ----------

pub fn db_path() -> Result<PathBuf, Box<dyn Error>> {
    let home = dirs::home_dir().ok_or("couldn't find your home folder")?;
    let dir = home.join(".liftlog");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("liftlog.db"))
}

pub fn open_db() -> Result<Connection, Box<dyn Error>> {
    let conn = Connection::open(db_path()?)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS exercises (
            id   INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
        );
        CREATE TABLE IF NOT EXISTS sets (
            id          INTEGER PRIMARY KEY,
            exercise_id INTEGER NOT NULL REFERENCES exercises(id),
            logged_on   TEXT NOT NULL,
            set_number  INTEGER NOT NULL,
            weight      REAL NOT NULL,
            reps        INTEGER NOT NULL
        );",
    )?;
    Ok(conn)
}

pub fn exercise_names(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM exercises ORDER BY name")?;
    let names = stmt.query_map([], |row| row.get(0))?;
    names.collect()
}

pub fn exercise_id(conn: &Connection, name: &str) -> rusqlite::Result<i64> {
    conn.execute("INSERT OR IGNORE INTO exercises (name) VALUES (?1)", params![name])?;
    conn.query_row("SELECT id FROM exercises WHERE name = ?1", params![name], |row| row.get(0))
}

pub fn log_sets(conn: &mut Connection, exercise: &str, sets: &[Set], date: NaiveDate) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let ex_id = exercise_id(&tx, &exercise.to_lowercase())?;
    let today = date.to_string();

    for (i, set) in sets.iter().enumerate() {
        tx.execute(
            "INSERT INTO sets (exercise_id, logged_on, set_number, weight, reps)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ex_id, today, i as i64 + 1, set.weight, set.reps],
        )?;
    }
    tx.commit()
}

// ---------- pure helpers ----------

pub fn parse_set(s: &str) -> Result<Set, String> {
    let (w, r) = s
        .split_once('x')
        .ok_or_else(|| format!("'{s}' should look like 315x5"))?;
    let weight = w.parse::<f64>().map_err(|_| format!("Bad weight in '{s}'"))?;
    let reps = r.parse::<u32>().map_err(|_| format!("Bad reps in '{s}'"))?;
    Ok(Set { weight, reps })
}

/// Epley estimated 1RM. A single is just the weight.
pub fn e1rm(set: &Set) -> f64 {
    if set.reps == 1 {
        set.weight
    } else {
        set.weight * (1.0 + set.reps as f64 / 30.0)
    }
}

pub fn increment(exercise: &str) -> f64 {
    if exercise.contains("squat") || exercise.contains("deadlift") {
        10.0
    } else {
        5.0
    }
}

// ---------- next ----------

pub fn last_session(conn: &Connection, exercise: &str) -> rusqlite::Result<Vec<Set>> {
    let mut stmt = conn.prepare(
        "SELECT s.weight, s.reps
         FROM sets s JOIN exercises e ON e.id = s.exercise_id
         WHERE e.name = ?1
           AND s.logged_on = (SELECT MAX(s2.logged_on) FROM sets s2
                              WHERE s2.exercise_id = e.id)
         ORDER BY s.set_number",
    )?;
    let rows = stmt.query_map(params![exercise], |row| {
        Ok(Set { weight: row.get(0)?, reps: row.get(1)? })
    })?;
    rows.collect()
}

/// Double progression in the 3-6 rep range. None if the lift was never logged.
pub fn suggest_next(conn: &Connection, exercise: &str) -> rusqlite::Result<Option<Suggestion>> {
    let name = exercise.to_lowercase();
    let sets = last_session(conn, &name)?;
    if sets.is_empty() {
        return Ok(None);
    }

    let top = sets.iter().map(|s| s.weight).fold(f64::MIN, f64::max);
    let working: Vec<&Set> = sets.iter().filter(|s| s.weight == top).collect();
    let fewest = match working.iter().map(|s| s.reps).min() {
        Some(r) => r,
        None => return Ok(None),
    };
    let step = increment(&name);

    let (action, next_weight, target_reps, message) = if fewest >= MAX_REPS {
        let w = top + step;
        (Action::Increase, w, MIN_REPS,
         format!("Next: {w} lbs, aim for {MIN_REPS}+ reps every set. You earned the jump."))
    } else if fewest < MIN_REPS {
        let w = top - step;
        (Action::Decrease, w, MIN_REPS,
         format!("Next: {w} lbs, aim for {MIN_REPS} reps every set. Last weight was too heavy for the range."))
    } else {
        (Action::Repeat, top, fewest + 1,
         format!("Next: {top} lbs again, aim for {} reps every set.", fewest + 1))
    };

    Ok(Some(Suggestion {
        exercise: name,
        last_weight: top,
        working_sets: working.len(),
        fewest_reps: fewest,
        next_weight,
        target_reps,
        action,
        message,
    }))
}

// ---------- prs ----------

/// Heaviest actual weight per exercise (ties go to more reps), sorted by name.
pub fn prs(conn: &Connection) -> rusqlite::Result<Vec<Pr>> {
    let mut stmt = conn.prepare(
        "SELECT e.name, s.logged_on, s.weight, s.reps
         FROM sets s JOIN exercises e ON e.id = s.exercise_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            Set { weight: row.get(2)?, reps: row.get(3)? },
        ))
    })?;

    let mut best: HashMap<String, (Set, String)> = HashMap::new();
    for row in rows {
        let (name, date, set) = row?;
        let is_better = match best.get(&name) {
            Some((current, _)) => {
                set.weight > current.weight
                    || (set.weight == current.weight && set.reps > current.reps)
            }
            None => true,
        };
        if is_better {
            best.insert(name, (set, date));
        }
    }

    let mut list: Vec<Pr> = best
        .into_iter()
        .map(|(exercise, (set, date))| Pr { exercise, weight: set.weight, reps: set.reps, date })
        .collect();
    list.sort_by(|a, b| a.exercise.cmp(&b.exercise));
    Ok(list)
}

// ---------- history ----------

/// Every session for an exercise, newest first, sets in the order they were done.
pub fn history(conn: &Connection, exercise: &str) -> rusqlite::Result<Vec<HistoryDay>> {
    let mut stmt = conn.prepare(
        "SELECT s.logged_on, s.weight, s.reps
         FROM sets s JOIN exercises e ON e.id = s.exercise_id
         WHERE e.name = ?1
         ORDER BY s.logged_on DESC, s.set_number ASC",
    )?;
    let rows = stmt.query_map(params![exercise.to_lowercase()], |row| {
        Ok((row.get::<_, String>(0)?, Set { weight: row.get(1)?, reps: row.get(2)? }))
    })?;

    let mut days: Vec<HistoryDay> = Vec::new();
    for row in rows {
        let (date, set) = row?;
        match days.last_mut() {
            Some(day) if day.date == date => day.sets.push(set),
            _ => days.push(HistoryDay { date, sets: vec![set] }),
        }
    }
    Ok(days)
}

// ---------- trend ----------

/// Best e1RM per session date, oldest first.
pub fn session_bests(conn: &Connection, exercise: &str) -> rusqlite::Result<BTreeMap<String, f64>> {
    let mut stmt = conn.prepare(
        "SELECT s.logged_on, s.weight, s.reps
         FROM sets s JOIN exercises e ON e.id = s.exercise_id
         WHERE e.name = ?1",
    )?;
    let rows = stmt.query_map(params![exercise], |row| {
        Ok((row.get::<_, String>(0)?, Set { weight: row.get(1)?, reps: row.get(2)? }))
    })?;

    let mut bests: BTreeMap<String, f64> = BTreeMap::new();
    for row in rows {
        let (date, set) = row?;
        let est = e1rm(&set);
        let best = bests.entry(date).or_insert(0.0);
        if est > *best {
            *best = est;
        }
    }
    Ok(bests)
}

/// Compare the latest session to the best session in the 14 days before it.
fn cut_check(bests: &BTreeMap<String, f64>) -> Option<CutCheck> {
    let (last_date, last_est) = bests.iter().next_back()?;
    let last_day: NaiveDate = last_date.parse().ok()?;
    let window_start = last_day - Days::new(14);

    let recent_peak = bests
        .iter()
        .filter_map(|(d, e)| d.parse::<NaiveDate>().ok().map(|day| (day, *e)))
        .filter(|(day, _)| *day >= window_start && *day < last_day)
        .map(|(_, e)| e)
        .fold(f64::MIN, f64::max);

    if recent_peak == f64::MIN {
        return None;
    }

    let change = (last_est - recent_peak) / recent_peak * 100.0;
    let (status, message) = if change <= -5.0 {
        (CutStatus::Warning, format!(
            "⚠ Down {:.1}% from your 2-week best ({recent_peak:.0}). If this shows up again next session, your cut may be costing strength.",
            -change
        ))
    } else if change < 0.0 {
        (CutStatus::SlightDip, format!(
            "Slight dip ({change:.1}%) vs your 2-week best. Normal day-to-day variation."
        ))
    } else {
        (CutStatus::Holding, format!("Holding strong: {change:+.1}% vs your 2-week best."))
    };

    Some(CutCheck { recent_peak, change_pct: change, status, message })
}

pub fn trend(conn: &Connection, exercise: &str) -> rusqlite::Result<Trend> {
    let name = exercise.to_lowercase();
    let bests = session_bests(conn, &name)?;
    let cut_check = cut_check(&bests);
    let points = bests
        .into_iter()
        .map(|(date, e1rm)| TrendPoint { date, e1rm })
        .collect();
    Ok(Trend { exercise: name, points, cut_check })
}
