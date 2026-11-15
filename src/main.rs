use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Input, Select};
use liftlog::*;
use rusqlite::Connection;
use std::error::Error;

#[derive(Parser)]
#[command(name = "liftlog", about = "Track lifts and get progression suggestions")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Log sets for an exercise, e.g. `liftlog log bench 315x5 305x6`
    Log {
        exercise: String,
        #[arg(required = true, value_parser = parse_set)]
        sets: Vec<Set>,
        /// Date of the session (YYYY-MM-DD), defaults to today
        #[arg(long)]
        date: Option<NaiveDate>,
    },
    /// Show recent history for an exercise
    History { exercise: String },
    /// Show your best estimated 1RM for each lift
    Prs,
    /// Suggest weight and reps for your next session
    Next { exercise: String },
    /// Show your est. 1RM trend and a strength-retention check
    Trend { exercise: String },
}

/// Pick from exercises you've logged before, or type a new one
fn pick_exercise(conn: &Connection, theme: &ColorfulTheme) -> Result<String, Box<dyn Error>> {
    let mut options = exercise_names(conn)?;
    options.push("+ New exercise".to_string());

    let pick = Select::with_theme(theme)
        .with_prompt("Exercise")
        .items(&options)
        .default(0)
        .interact()?;

    if pick == options.len() - 1 {
        let name: String = Input::with_theme(theme)
            .with_prompt("Exercise name")
            .interact_text()?;
        Ok(name.trim().to_lowercase())
    } else {
        Ok(options[pick].clone())
    }
}

fn interactive(conn: &mut Connection) -> Result<(), Box<dyn Error>> {
    let theme = ColorfulTheme::default();
    let choices = ["Log a workout", "Next session target", "History", "Trend", "PRs", "Quit"];

    loop {
        let pick = Select::with_theme(&theme)
            .with_prompt("What do you want to do?")
            .items(&choices)
            .default(0)
            .interact()?;

        match pick {
            0 => {
                let exercise = pick_exercise(conn, &theme)?;
                print_next(conn, &exercise)?; // show today's target first

                let mut sets = Vec::new();
                loop {
                    let input: String = Input::with_theme(&theme)
                        .with_prompt(format!("Set {} (e.g. 315x5, Enter to finish)", sets.len() + 1))
                        .allow_empty(true)
                        .interact_text()?;

                    let input = input.trim();
                    if input.is_empty() {
                        break;
                    }
                    match parse_set(input) {
                        Ok(set) => sets.push(set),
                        Err(e) => println!("{e}"),
                    }
                }

                if sets.is_empty() {
                    println!("Nothing logged.");
                } else {
                    log_sets(conn, &exercise, &sets, Local::now().date_naive())?;
                    println!("Saved {} sets of {exercise}.", sets.len());
                }
            }
            1 => {
                let exercise = pick_exercise(conn, &theme)?;
                print_next(conn, &exercise)?;
            }
            2 => {
                let exercise = pick_exercise(conn, &theme)?;
                print_history(conn, &exercise)?;
            }
            3 => {
                let exercise = pick_exercise(conn, &theme)?;
                print_trend(conn, &exercise)?;
            }
            4 => print_prs(conn)?,
            _ => break,
        }
        println!();
    }
    Ok(())
}

fn print_next(conn: &Connection, exercise: &str) -> rusqlite::Result<()> {
    match suggest_next(conn, exercise)? {
        None => println!("No sets logged for {} yet.", exercise.to_lowercase()),
        Some(s) => {
            println!(
                "Last {}: {} set(s) at {} lbs, fewest reps {}",
                s.exercise, s.working_sets, s.last_weight, s.fewest_reps
            );
            println!("{}", s.message);
        }
    }
    Ok(())
}

fn print_prs(conn: &Connection) -> rusqlite::Result<()> {
    let list = prs(conn)?;
    if list.is_empty() {
        println!("No lifts logged yet.");
        return Ok(());
    }
    for pr in &list {
        println!("{:<12} {} x {}  ({})", pr.exercise, pr.weight, pr.reps, pr.date);
    }
    Ok(())
}

fn print_history(conn: &Connection, exercise: &str) -> rusqlite::Result<()> {
    let days = history(conn, exercise)?;
    if days.is_empty() {
        println!("No sets logged for {exercise} yet.");
        return Ok(());
    }
    for day in &days {
        println!("\n{}", day.date);
        for set in &day.sets {
            println!(" {} x {} (e1RM {:.0})", set.weight, set.reps, e1rm(set));
        }
    }
    Ok(())
}

fn print_trend(conn: &Connection, exercise: &str) -> rusqlite::Result<()> {
    let t = trend(conn, exercise)?;
    if t.points.is_empty() {
        println!("No sets logged for {} yet.", t.exercise);
        return Ok(());
    }

    let high = t.points.iter().map(|p| p.e1rm).fold(f64::MIN, f64::max);
    let lowest = t.points.iter().map(|p| p.e1rm).fold(f64::MAX, f64::min);
    let low = lowest.min(high * 0.85);
    let range = (high - low).max(1.0);

    println!("{}: best est. 1RM per session\n", t.exercise);
    for p in &t.points {
        let bar_len = 5 + ((p.e1rm - low) / range * 25.0).round() as usize;
        println!("{}  {:>4.0}  {}", p.date, p.e1rm, "▇".repeat(bar_len));
    }

    println!();
    match &t.cut_check {
        None => println!("Not enough sessions in the last 2 weeks to compare yet."),
        Some(check) => println!("{}", check.message),
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let mut conn = open_db()?;

    match cli.command {
        None => interactive(&mut conn)?,
        Some(Command::Log { exercise, sets, date }) => {
            let date = date.unwrap_or_else(|| Local::now().date_naive());
            log_sets(&mut conn, &exercise, &sets, date)?;
            for set in &sets {
                println!("{exercise}: {} lbs x {} (est. 1RM {:.0})", set.weight, set.reps, e1rm(set));
            }
            println!("Saved {} sets.", sets.len());
        }
        Some(Command::History { exercise }) => print_history(&conn, &exercise)?,
        Some(Command::Prs) => print_prs(&conn)?,
        Some(Command::Next { exercise }) => print_next(&conn, &exercise)?,
        Some(Command::Trend { exercise }) => print_trend(&conn, &exercise)?,
    }
    Ok(())
}
